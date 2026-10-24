// Small static pages served by the edge. Plain HTML + inline CSS (light/dark), no scripts.

import { CONTINUE_PATH } from "./interstitial";
import { NOINDEX } from "./http";

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

const CSS = `
:root{color-scheme:light dark;--bg:#f7f7f8;--card:#fff;--fg:#1d1d1f;--muted:#5f6368;--border:#e3e3e6;--accent:#4f46e5;--warn:#b45309;--warn-bg:#fef3c7}
@media (prefers-color-scheme:dark){:root{--bg:#111113;--card:#1a1a1d;--fg:#ececef;--muted:#a0a0a8;--border:#2c2c31;--accent:#818cf8;--warn:#fbbf24;--warn-bg:#3a2a07}}
*{box-sizing:border-box}body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:var(--bg);color:var(--fg);font:15px/1.55 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;padding:16px}
main{max-width:560px;width:100%;background:var(--card);border:1px solid var(--border);border-radius:12px;padding:28px 28px 22px}
h1{font-size:20px;margin:0 0 10px}p{margin:0 0 12px;color:var(--muted)}code{font:13px ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;background:var(--bg);border:1px solid var(--border);border-radius:5px;padding:1px 5px;word-break:break-all}
.warn{background:var(--warn-bg);color:var(--warn);border-radius:8px;padding:10px 12px;margin:0 0 14px;font-weight:500}
.actions{display:flex;gap:10px;flex-wrap:wrap;margin-top:18px}
a.btn{display:inline-block;padding:8px 14px;border-radius:8px;text-decoration:none;font-weight:600;border:1px solid var(--border);color:var(--fg)}
a.btn.primary{background:var(--accent);border-color:var(--accent);color:#fff}
footer{margin-top:18px;font-size:12px;color:var(--muted)}footer a{color:var(--muted)}
`;

let sourceUrl: string | null = null;

/** AGPL §13: pages link to the running relay's source. Called with the env on each entry point. */
export function configurePages(env: { SOURCE_URL?: string }): void {
  sourceUrl = env.SOURCE_URL && /^https:\/\//.test(env.SOURCE_URL) ? env.SOURCE_URL : null;
}

function layout(title: string, body: string): string {
  const source = sourceUrl ? ` <a href="${escapeHtml(sourceUrl)}" rel="noopener">Source</a>.` : "";
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="robots" content="noindex,nofollow"><title>${escapeHtml(title)}</title><style>${CSS}</style></head><body><main>${body}<footer>Served by the /dev/all relay — open source (AGPL-3.0) and self-hostable.${source}</footer></main></body></html>`;
}

export function htmlResponse(status: number, title: string, body: string, headers: HeadersInit = {}): Response {
  const h = new Headers(headers);
  h.set("content-type", "text/html; charset=utf-8");
  h.set("cache-control", "no-store");
  h.set("x-robots-tag", NOINDEX);
  h.set("content-security-policy", "default-src 'none'; style-src 'unsafe-inline'; form-action 'none'; frame-ancestors 'none'");
  h.set("referrer-policy", "no-referrer");
  return new Response(layout(title, body), { status, headers: h });
}

function wantsHtml(request: Request): boolean {
  return (request.headers.get("accept") ?? "").includes("text/html");
}

/** Error page for browsers, short text for everything else (webhook senders, curl). */
export function problem(request: Request, status: number, title: string, message: string, headers: HeadersInit = {}): Response {
  if (wantsHtml(request)) return htmlResponse(status, title, `<h1>${escapeHtml(title)}</h1><p>${escapeHtml(message)}</p>`, headers);
  const h = new Headers(headers);
  h.set("content-type", "text/plain; charset=utf-8");
  h.set("cache-control", "no-store");
  h.set("x-robots-tag", NOINDEX);
  return new Response(`${title}: ${message}\n`, { status, headers: h });
}

export function offlinePage(request: Request, host: string): Response {
  return problem(
    request,
    502,
    "This tunnel is offline",
    `Nothing is connected to ${host} right now. If this is your tunnel, start it again from /dev/all (Tunnels page) — the address stays the same while your session lasts.`,
    { "retry-after": "10" },
  );
}

export function interstitialPage(request: Request, host: string, reportUrl: string): Response {
  const url = new URL(request.url);
  const target = url.pathname + url.search;
  const cont = `${CONTINUE_PATH}?to=${encodeURIComponent(target)}`;
  const body = `
<h1>You are about to visit ${escapeHtml(host)}</h1>
<p class="warn">This site is served from someone's computer through a free /dev/all tunnel.</p>
<p>Only continue if you know who sent you this link. <strong>Do not enter passwords, payment details or other sensitive information</strong> unless you trust the person running it — /dev/all does not review what tunnels serve.</p>
<p>Developers: API clients, webhooks and anything that isn't a browser page load never see this page. Send the <code>devall-skip-browser-warning: 1</code> header to skip it, or upgrade to Supporter to remove it.</p>
<div class="actions"><a class="btn primary" href="${escapeHtml(cont)}">Continue to the site</a><a class="btn" href="${escapeHtml(reportUrl)}" rel="noopener">Report abuse</a></div>`;
  return htmlResponse(200, `Visit ${host}?`, body);
}

export function quotaPage(request: Request, what: string, resetAt: number | null): Response {
  const headers: Record<string, string> = {};
  if (resetAt) headers["retry-after"] = String(Math.max(1, Math.ceil((resetAt - Date.now()) / 1000)));
  return problem(request, 429, "Tunnel quota reached", `${what} The owner can upgrade to Supporter or self-host the relay for free.`, headers);
}

export function blockedPage(request: Request): Response {
  return problem(request, 451, "Tunnel disabled", "This address was disabled after an abuse report.");
}

export function reportPage(host: string | null, abuseEmail: string): Response {
  const subject = encodeURIComponent(`Abuse report: ${host ?? "(tunnel address)"}`);
  const body = encodeURIComponent(`Address: ${host ?? ""}\nWhat happened:\n`);
  const mail = `mailto:${abuseEmail}?subject=${subject}&body=${body}`;
  return htmlResponse(
    200,
    "Report abuse",
    `<h1>Report a /dev/all tunnel</h1>
<p>Tunnels and webhook inboxes are run by individual developers. If ${host ? `<code>${escapeHtml(host)}</code>` : "an address"} is used for phishing, malware or anything illegal, tell us and we will disable it — usually within hours.</p>
<p>Please include the full address and what you saw. We never ask for passwords.</p>
<div class="actions"><a class="btn primary" href="${escapeHtml(mail)}">Email ${escapeHtml(abuseEmail)}</a></div>`,
  );
}

export function landingPage(domain: string): Response {
  return htmlResponse(
    200,
    "/dev/all relay",
    `<h1>/dev/all relay</h1>
<p>This host forwards tunnels and webhook inboxes to developers' machines running /dev/all. Addresses under <code>${escapeHtml(domain)}</code> belong to individual users.</p>
<div class="actions"><a class="btn" href="/report">Report abuse</a></div>`,
  );
}
