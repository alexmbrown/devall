// ShareDO — one shared request/response snapshot (https://share.<domain>/<id>), read-only and
// expiring. The desktop redacts secrets *before* uploading (secret values, Authorization / Cookie /
// API-key headers unless the user unticks that); the relay never sees what was masked.

import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";
import { NOINDEX } from "./http";
import { escapeHtml } from "./pages";

/** Rows are split so no SQLite value comes near the 2 MB row limit. */
const CHUNK = 512 * 1024;

export interface SnapshotRequest {
  method: string;
  url: string;
  headers: [string, string][];
  body?: string | null;
  bodyTruncated?: boolean;
}

export interface SnapshotResponse {
  status: number;
  statusText?: string | null;
  headers: [string, string][];
  /** UTF-8 body (text responses). */
  body?: string | null;
  /** Binary bodies. */
  bodyBase64?: string | null;
  bodyTruncated?: boolean;
  size?: number | null;
  contentType?: string | null;
}

/** What the desktop uploads (`POST /v1/shares`). */
export interface Snapshot {
  version: 1;
  title: string;
  request: SnapshotRequest;
  response?: SnapshotResponse | null;
  error?: string | null;
  timing?: { totalMs?: number; ttfbMs?: number | null } | null;
  environment?: string | null;
  /** Unix ms when the request was sent. */
  at?: number | null;
  /** cURL command (already redacted) for "Copy as cURL". */
  curl?: string | null;
  /** `.req.yaml` (already redacted) for "Download". */
  reqYaml?: string | null;
  /** Which kinds of values the desktop masked (shown on the page). */
  redactions?: string[];
}

export interface ShareMeta {
  id: string;
  owner: string;
  title: string;
  createdAt: number;
  expiresAt: number;
  bytes: number;
  chunks: number;
  views: number;
}

const enc = new TextEncoder();
const dec = new TextDecoder();

/** Validates the uploaded snapshot's shape (bounded strings, header pairs). Returns an error message or null. */
export function validateSnapshot(v: unknown): string | null {
  if (!v || typeof v !== "object" || Array.isArray(v)) return "snapshot must be an object";
  const s = v as Partial<Snapshot>;
  if (s.version !== 1) return "snapshot.version must be 1";
  if (typeof s.title !== "string" || s.title.length > 200) return "snapshot.title must be a string (≤ 200 chars)";
  const r = s.request;
  if (!r || typeof r !== "object" || typeof r.method !== "string" || typeof r.url !== "string") return "snapshot.request needs method and url";
  const pairs = (h: unknown) => Array.isArray(h) && h.length <= 500 && h.every((p) => Array.isArray(p) && p.length === 2 && typeof p[0] === "string" && typeof p[1] === "string");
  if (!pairs(r.headers)) return "snapshot.request.headers must be [name, value] pairs";
  if (s.response) {
    if (typeof s.response.status !== "number" || !pairs(s.response.headers)) return "snapshot.response needs status and [name, value] headers";
  }
  return null;
}

export class ShareDO extends DurableObject<Env> {
  private sql: SqlStorage;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS chunks (i INTEGER PRIMARY KEY, data BLOB NOT NULL)");
  }

  private meta(): Promise<ShareMeta | undefined> {
    return this.ctx.storage.get<ShareMeta>("meta");
  }

  async create(args: { id: string; owner: string; snapshot: Snapshot; ttlSeconds: number }): Promise<ShareMeta> {
    const bytes = enc.encode(JSON.stringify(args.snapshot));
    const now = Date.now();
    const chunks = Math.max(1, Math.ceil(bytes.byteLength / CHUNK));
    this.ctx.storage.transactionSync(() => {
      this.sql.exec("DELETE FROM chunks");
      for (let i = 0; i < chunks; i++) this.sql.exec("INSERT INTO chunks (i, data) VALUES (?, ?)", i, bytes.slice(i * CHUNK, (i + 1) * CHUNK).buffer);
    });
    const meta: ShareMeta = { id: args.id, owner: args.owner, title: args.snapshot.title, createdAt: now, expiresAt: now + args.ttlSeconds * 1000, bytes: bytes.byteLength, chunks, views: 0 };
    await this.ctx.storage.put("meta", meta);
    await this.ctx.storage.setAlarm(meta.expiresAt);
    return meta;
  }

  async revoke(owner: string): Promise<boolean> {
    const meta = await this.meta();
    if (!meta || meta.owner !== owner) return false;
    await this.ctx.storage.deleteAll();
    return true;
  }

  async info(): Promise<ShareMeta | null> {
    const meta = await this.meta();
    if (!meta || meta.expiresAt <= Date.now()) return null;
    return meta;
  }

  override async alarm(): Promise<void> {
    const meta = await this.meta();
    if (!meta || meta.expiresAt <= Date.now() + 1000) await this.ctx.storage.deleteAll();
    else await this.ctx.storage.setAlarm(meta.expiresAt);
  }

  private snapshot(): Snapshot {
    const parts = this.sql.exec("SELECT data FROM chunks ORDER BY i").toArray().map((r) => new Uint8Array(r.data as ArrayBuffer));
    const total = parts.reduce((n, p) => n + p.byteLength, 0);
    const all = new Uint8Array(total);
    let off = 0;
    for (const p of parts) {
      all.set(p, off);
      off += p.byteLength;
    }
    return JSON.parse(dec.decode(all)) as Snapshot;
  }

  /** Public viewer: `/<id>`, `/<id>/raw.json`, `/<id>/request.req.yaml`, `/<id>/curl.sh`. */
  override async fetch(request: Request): Promise<Response> {
    if (request.method !== "GET" && request.method !== "HEAD") return new Response("Method not allowed\n", { status: 405, headers: { allow: "GET, HEAD" } });
    const meta = await this.meta();
    if (!meta || meta.expiresAt <= Date.now()) return gone();
    const url = new URL(request.url);
    const rest = url.pathname.split("/").slice(2).join("/");
    const snap = this.snapshot();
    if (rest === "" || rest === "/") {
      meta.views += 1;
      await this.ctx.storage.put("meta", meta);
      return viewerPage(snap, meta, url);
    }
    const slug = (snap.title || "request").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 60) || "request";
    if (rest === "raw.json") return download(JSON.stringify(snap, null, 2), "application/json", `${slug}.share.json`);
    if (rest === "request.req.yaml" && snap.reqYaml) return download(snap.reqYaml, "text/yaml", `${slug}.req.yaml`);
    if (rest === "curl.sh" && snap.curl) return download(`${snap.curl}\n`, "text/plain", `${slug}.sh`);
    return new Response("Not found\n", { status: 404, headers: { "x-robots-tag": NOINDEX } });
  }
}

function download(body: string, type: string, filename: string): Response {
  return new Response(body, {
    headers: {
      "content-type": `${type}; charset=utf-8`,
      "content-disposition": `attachment; filename="${filename}"`,
      "x-robots-tag": NOINDEX,
      "cache-control": "private, no-store",
      "x-content-type-options": "nosniff",
      "content-security-policy": "sandbox; default-src 'none'",
    },
  });
}

export function gone(): Response {
  return pageResponse(
    410,
    "Link expired",
    `<main class="narrow"><h1>This link has expired</h1><p class="muted">Shared /dev/all snapshots expire automatically, and their owner can revoke them earlier. Ask whoever sent it for a new link.</p></main>`,
    null,
  );
}

const CSS = `
:root{color-scheme:light dark;--bg:#f7f7f8;--card:#fff;--fg:#1d1d1f;--muted:#5f6368;--border:#e3e3e6;--accent:#4f46e5;--code:#f1f1f4;--ok:#15803d;--warn:#b45309;--err:#b91c1c}
@media (prefers-color-scheme:dark){:root{--bg:#111113;--card:#1a1a1d;--fg:#ececef;--muted:#a0a0a8;--border:#2c2c31;--accent:#818cf8;--code:#141417;--ok:#4ade80;--warn:#fbbf24;--err:#f87171}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;padding:24px 16px}
main{max-width:1000px;margin:0 auto;display:flex;flex-direction:column;gap:16px}main.narrow{max-width:560px}
h1{font-size:20px;margin:0}h2{font-size:15px;margin:0 0 8px}.muted{color:var(--muted)}
.card{background:var(--card);border:1px solid var(--border);border-radius:12px;padding:16px;min-width:0}
.line{display:flex;gap:10px;align-items:center;flex-wrap:wrap}.method{font-weight:700;font-family:ui-monospace,Menlo,Consolas,monospace}
.url{font-family:ui-monospace,Menlo,Consolas,monospace;word-break:break-all}
.status{font-weight:700}.s2{color:var(--ok)}.s3{color:var(--accent)}.s4{color:var(--warn)}.s5{color:var(--err)}
table{border-collapse:collapse;width:100%;font-size:13px}td{border-top:1px solid var(--border);padding:4px 8px;vertical-align:top;word-break:break-all}td:first-child{white-space:nowrap;color:var(--muted);width:1%;word-break:normal}
pre{margin:0;background:var(--code);border:1px solid var(--border);border-radius:8px;padding:10px;overflow:auto;max-height:520px;font:12.5px/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;white-space:pre-wrap;word-break:break-word}
.actions{display:flex;gap:8px;flex-wrap:wrap}.btn{display:inline-block;padding:6px 12px;border-radius:8px;border:1px solid var(--border);background:var(--card);color:var(--fg);text-decoration:none;font:inherit;font-weight:600;cursor:pointer}
.btn.primary{background:var(--accent);border-color:var(--accent);color:#fff}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(320px,1fr));gap:16px}
.badge{border:1px solid var(--border);border-radius:999px;padding:1px 8px;font-size:12px;color:var(--muted)}
footer{font-size:12px;color:var(--muted);text-align:center}footer a{color:var(--muted)}
`;

function pageResponse(status: number, title: string, body: string, nonce: string | null): Response {
  const script = nonce ? `'nonce-${nonce}'` : "'none'";
  return new Response(
    `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="robots" content="noindex,nofollow"><title>${escapeHtml(title)}</title><style>${CSS}</style></head><body>${body}</body></html>`,
    {
      status,
      headers: {
        "content-type": "text/html; charset=utf-8",
        "cache-control": "private, no-store",
        "x-robots-tag": NOINDEX,
        "referrer-policy": "no-referrer",
        "x-content-type-options": "nosniff",
        "content-security-policy": `default-src 'none'; style-src 'unsafe-inline'; script-src ${script}; form-action 'none'; frame-ancestors 'none'; base-uri 'none'`,
      },
    },
  );
}

function headersTable(h: [string, string][]): string {
  if (!h.length) return `<p class="muted">No headers.</p>`;
  return `<table>${h.map(([k, v]) => `<tr><td>${escapeHtml(k)}</td><td>${escapeHtml(v)}</td></tr>`).join("")}</table>`;
}

function pretty(body: string, contentType: string | null | undefined): string {
  const t = body.trim();
  if ((contentType ?? "").includes("json") || t.startsWith("{") || t.startsWith("[")) {
    try {
      return JSON.stringify(JSON.parse(t), null, 2);
    } catch {
      // not JSON after all
    }
  }
  return body;
}

function bodyBlock(body: string | null | undefined, b64: string | null | undefined, contentType: string | null | undefined, truncated: boolean | undefined, size: number | null | undefined): string {
  if (b64) return `<p class="muted">Binary body (${size ?? Math.floor((b64.length * 3) / 4)} bytes). Download the raw JSON to get it.</p>`;
  if (body === null || body === undefined || body === "") return `<p class="muted">No body.</p>`;
  const note = truncated ? `<p class="muted">Truncated.</p>` : "";
  return `<pre>${escapeHtml(pretty(body, contentType))}</pre>${note}`;
}

function statusClass(s: number): string {
  return s >= 500 ? "s5" : s >= 400 ? "s4" : s >= 300 ? "s3" : "s2";
}

function viewerPage(s: Snapshot, meta: ShareMeta, url: URL): Response {
  const nonce = crypto.randomUUID().replace(/-/g, "");
  const base = url.pathname.replace(/\/+$/, "");
  const res = s.response;
  const reqCt = s.request.headers.find(([k]) => k.toLowerCase() === "content-type")?.[1];
  const when = s.at ? new Date(s.at).toISOString().replace("T", " ").slice(0, 19) + " UTC" : null;
  const expires = new Date(meta.expiresAt).toISOString().slice(0, 10);
  const timing = s.timing?.totalMs !== undefined ? `<span class="badge">${Math.round(s.timing.totalMs)} ms</span>` : "";
  const redactions = s.redactions?.length ? `<p class="muted">Masked before sharing: ${s.redactions.map(escapeHtml).join(", ")}.</p>` : "";
  const curl = s.curl
    ? `<div class="card"><h2>cURL</h2><pre id="curl">${escapeHtml(s.curl)}</pre><div class="actions" style="margin-top:8px"><button class="btn primary" id="copy" type="button">Copy as cURL</button><a class="btn" href="${escapeHtml(base)}/curl.sh">Download .sh</a></div></div>`
    : "";
  const body = `<main>
<div class="line"><h1>${escapeHtml(s.title || "Shared request")}</h1><span class="badge">read-only</span>${s.environment ? `<span class="badge">${escapeHtml(s.environment)}</span>` : ""}${timing}</div>
<p class="muted">Shared from /dev/all${when ? ` · sent ${escapeHtml(when)}` : ""} · expires ${escapeHtml(expires)}</p>
${redactions}
<div class="card"><div class="line"><span class="method">${escapeHtml(s.request.method)}</span><span class="url">${escapeHtml(s.request.url)}</span></div>
${res ? `<div class="line" style="margin-top:8px"><span class="status ${statusClass(res.status)}">${res.status}${res.statusText ? ` ${escapeHtml(res.statusText)}` : ""}</span>${res.contentType ? `<span class="badge">${escapeHtml(res.contentType)}</span>` : ""}${typeof res.size === "number" ? `<span class="badge">${res.size} bytes</span>` : ""}</div>` : ""}
${s.error ? `<p class="s5" style="margin:8px 0 0">${escapeHtml(s.error)}</p>` : ""}</div>
<div class="grid">
<div class="card"><h2>Request headers</h2>${headersTable(s.request.headers)}<h2 style="margin-top:12px">Request body</h2>${bodyBlock(s.request.body, null, reqCt, s.request.bodyTruncated, null)}</div>
${res ? `<div class="card"><h2>Response headers</h2>${headersTable(res.headers)}<h2 style="margin-top:12px">Response body</h2>${bodyBlock(res.body, res.bodyBase64, res.contentType, res.bodyTruncated, res.size)}</div>` : ""}
</div>
${curl}
<div class="card"><h2>Open in /dev/all</h2><p class="muted">Download the request file into your project's <code>http/</code> folder, or paste the cURL command into the URL bar of a new request.</p><div class="actions">${s.reqYaml ? `<a class="btn primary" href="${escapeHtml(base)}/request.req.yaml">Download .req.yaml</a>` : ""}<a class="btn" href="${escapeHtml(base)}/raw.json">Download JSON</a></div></div>
<footer>Served by the /dev/all relay — open source and self-hostable. Shared snapshots are read-only and expire.</footer>
</main>
<script nonce="${nonce}">document.getElementById("copy")?.addEventListener("click",function(){var t=document.getElementById("curl").textContent;navigator.clipboard.writeText(t).then(function(){var b=document.getElementById("copy");b.textContent="Copied";setTimeout(function(){b.textContent="Copy as cURL"},1500)})});</script>`;
  return pageResponse(200, s.title || "Shared request", body, nonce);
}

export function snapshotBytes(s: Snapshot): number {
  return enc.encode(JSON.stringify(s)).byteLength;
}
