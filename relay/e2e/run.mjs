#!/usr/bin/env node
// Live end-to-end checks of the /dev/all relay: the real Worker under `wrangler dev --local`
// (self-host mode, static token, fully offline) driven by the real Rust CLI and plain
// HTTP/WebSocket clients. Prints PASS/FAIL per scenario and always stops what it started.
//
//   node relay/e2e/run.mjs                 # every scenario
//   node relay/e2e/run.mjs tunnel tcp      # only scenarios whose id contains one of the words
//   KEEP_TMP=1 node relay/e2e/run.mjs      # keep the temp folder (wrangler log, projects)
//
// Needs: `npm install` in relay/worker (wrangler) and `cargo build -p devall-cli`
// (built automatically when target/debug/devall is missing). No network access, no login.

import { spawn, execFileSync } from "node:child_process";
import crypto from "node:crypto";
import { EventEmitter } from "node:events";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import zlib from "node:zlib";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, "..", "..");
const WORKER = path.join(ROOT, "relay", "worker");
const WIN = process.platform === "win32";
const CLI = path.join(ROOT, "target", "debug", WIN ? "devall.exe" : "devall");
const DOMAIN = "localhost.test";
const TOKEN = `e2e-${crypto.randomBytes(16).toString("hex")}`;
const OTHER_TOKEN = `other-${crypto.randomBytes(16).toString("hex")}`;
const FREE_TOKEN = `free-${crypto.randomBytes(16).toString("hex")}`;
/** Free-tier session length on the e2e relay (LIMITS_JSON), so expiry + reconnect can be watched. */
const FREE_SESSION_SECONDS = 8;
const ADMIN = `admin-${crypto.randomBytes(16).toString("hex")}`;
const FILTER = process.argv.slice(2).map((s) => s.toLowerCase());

// ---------- processes (every one we start is tracked and killed at the end) ----------

const procs = new Set();

function startProc(name, cmd, args, opts = {}) {
  const child = spawn(cmd, args, { cwd: opts.cwd ?? ROOT, env: { ...process.env, ...(opts.env ?? {}) }, stdio: ["ignore", "pipe", "pipe"], windowsHide: true, detached: !WIN });
  const rec = { name, child, lines: [], errLines: [], exited: null, log: opts.log ? fs.createWriteStream(opts.log) : null, emitter: new EventEmitter() };
  const feed = (key) => {
    let partial = "";
    return (d) => {
      rec.log?.write(d);
      partial += d.toString("utf8");
      const parts = partial.split(/\r?\n/);
      partial = parts.pop() ?? "";
      for (const l of parts) {
        rec[key].push(l);
        rec.emitter.emit("line");
      }
    };
  };
  child.stdout.on("data", feed("lines"));
  child.stderr.on("data", feed("errLines"));
  child.on("exit", (code, signal) => {
    rec.exited = { code, signal };
    rec.emitter.emit("line");
  });
  // Close our log file once the pipes are drained (an open handle would block deleting the temp dir).
  child.on("close", () => rec.log?.end());
  child.on("error", (e) => {
    rec.exited = { code: -1, signal: String(e) };
    rec.emitter.emit("line");
  });
  procs.add(rec);
  return rec;
}

function killTree(rec) {
  if (!rec.child.pid || rec.exited) return;
  try {
    if (WIN) execFileSync("taskkill", ["/PID", String(rec.child.pid), "/T", "/F"], { stdio: "ignore" });
    else process.kill(-rec.child.pid, "SIGKILL");
  } catch {
    // already gone
  }
}

async function stopProc(rec, graceful = false) {
  if (!rec || rec.exited) return;
  if (graceful && !WIN) {
    try {
      process.kill(-rec.child.pid, "SIGINT");
    } catch {
      // gone
    }
    if (await waitExit(rec, 5000)) return;
  }
  killTree(rec);
  await waitExit(rec, 10_000);
}

/** Waits until every log file we opened is flushed and closed. */
async function closeLogs() {
  await Promise.all(
    [...procs].map((rec) =>
      rec.log && !rec.log.closed
        ? new Promise((r) => {
            rec.log.once("close", r);
            rec.log.end();
            setTimeout(r, 3000);
          })
        : null,
    ),
  );
}

function waitExit(rec, ms) {
  return new Promise((resolve) => {
    if (rec.exited) return resolve(true);
    const t = setTimeout(() => resolve(false), ms);
    rec.child.once("exit", () => {
      clearTimeout(t);
      resolve(true);
    });
  });
}

/** Waits for a line (stdout, or stderr with `stream: "err"`) matching `pred`, from index `from`. */
function waitLine(rec, pred, { timeout = 30_000, from = 0, stream = "out", what = "output" } = {}) {
  const list = stream === "err" ? rec.errLines : rec.lines;
  return new Promise((resolve, reject) => {
    const check = () => {
      for (let i = from; i < list.length; i++) {
        const v = pred(list[i]);
        if (v) {
          cleanup();
          resolve({ value: v === true ? list[i] : v, index: i });
          return true;
        }
      }
      if (rec.exited) {
        cleanup();
        reject(new Error(`${rec.name} exited (${JSON.stringify(rec.exited)}) before ${what}\n--- stdout\n${rec.lines.slice(-15).join("\n")}\n--- stderr\n${rec.errLines.slice(-15).join("\n")}`));
        return true;
      }
      return false;
    };
    const t = setTimeout(() => {
      cleanup();
      reject(new Error(`timed out waiting for ${what} from ${rec.name}\n--- stdout\n${rec.lines.slice(-15).join("\n")}\n--- stderr\n${rec.errLines.slice(-15).join("\n")}`));
    }, timeout);
    const cleanup = () => {
      clearTimeout(t);
      rec.emitter.off("line", check);
    };
    rec.emitter.on("line", check);
    check();
  });
}

function jsonLine(pred) {
  return (l) => {
    if (!l.startsWith("{")) return false;
    try {
      const v = JSON.parse(l);
      return pred(v) ? v : false;
    } catch {
      return false;
    }
  };
}

function runCli(args, env, timeout = 120_000) {
  return new Promise((resolve) => {
    const rec = startProc(`devall ${args.slice(0, 3).join(" ")}`, CLI, args, { env });
    const t = setTimeout(() => killTree(rec), timeout);
    rec.child.on("exit", (code) => {
      clearTimeout(t);
      setTimeout(() => resolve({ code, stdout: rec.lines.join("\n"), stderr: rec.errLines.join("\n"), lines: rec.lines }), 50);
    });
  });
}

// ---------- small helpers ----------

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sha256 = (b) => crypto.createHash("sha256").update(b).digest("hex");

function assert(cond, msg) {
  if (!cond) throw new Error(`assertion failed: ${msg}`);
}

function eq(actual, expected, msg) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) throw new Error(`${msg}: expected ${e}, got ${a}`);
}

function freePort() {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.unref();
    s.on("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });
}

let RELAY_PORT = 0;

/** One HTTP request to the relay with an explicit Host header (no keep-alive). */
function relayReq({ method = "GET", host, path: p = "/", headers = {}, body = null, port = RELAY_PORT }) {
  return new Promise((resolve, reject) => {
    const h = { Host: host, ...headers };
    if (body !== null) h["Content-Length"] = String(Buffer.byteLength(body));
    const req = http.request({ host: "127.0.0.1", port, method, path: p, headers: h, agent: false }, (res) => {
      const chunks = [];
      res.on("data", (c) => chunks.push(c));
      res.on("end", () => resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks) }));
      res.on("error", reject);
    });
    req.on("error", reject);
    req.setTimeout(60_000, () => req.destroy(new Error(`timeout: ${method} ${host}${p}`)));
    if (body !== null) req.write(body);
    req.end();
  });
}

const relayHost = `relay.${DOMAIN}`;
const auth = (t = TOKEN) => ({ Authorization: `Bearer ${t}`, "Content-Type": "application/json" });

async function api(method, p, body, token = TOKEN) {
  const r = await relayReq({ method, host: relayHost, path: p, headers: auth(token), body: body === undefined ? null : JSON.stringify(body) });
  const text = r.body.toString("utf8");
  let json = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = { raw: text };
  }
  return { status: r.status, body: json, headers: r.headers };
}

// ---------- minimal WebSocket (RFC 6455) client + server framing ----------

const WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

class WsConn extends EventEmitter {
  constructor(socket, head, { server = false, protocol = null } = {}) {
    super();
    this.socket = socket;
    this.server = server;
    this.protocol = protocol;
    this.buf = head && head.length ? Buffer.from(head) : Buffer.alloc(0);
    this.queue = [];
    this.waiters = [];
    this.frag = null;
    this.closed = null;
    socket.setNoDelay(true);
    socket.on("data", (d) => {
      this.buf = Buffer.concat([this.buf, d]);
      this.parse();
    });
    socket.on("close", () => this.finish({ code: 1006, reason: "socket closed" }));
    socket.on("error", () => this.finish({ code: 1006, reason: "socket error" }));
    if (this.buf.length) setImmediate(() => this.parse());
  }

  finish(c) {
    if (!this.closed) this.closed = c;
    for (const w of this.waiters.splice(0)) w.reject(new Error(`websocket closed (${this.closed.code} ${this.closed.reason})`));
    this.emit("close", this.closed);
  }

  parse() {
    for (;;) {
      const b = this.buf;
      if (b.length < 2) return;
      const fin = (b[0] & 0x80) !== 0;
      const op = b[0] & 0x0f;
      const masked = (b[1] & 0x80) !== 0;
      let len = b[1] & 0x7f;
      let off = 2;
      if (len === 126) {
        if (b.length < 4) return;
        len = b.readUInt16BE(2);
        off = 4;
      } else if (len === 127) {
        if (b.length < 10) return;
        len = Number(b.readBigUInt64BE(2));
        off = 10;
      }
      let mask = null;
      if (masked) {
        if (b.length < off + 4) return;
        mask = b.subarray(off, off + 4);
        off += 4;
      }
      if (b.length < off + len) return;
      const payload = Buffer.from(b.subarray(off, off + len));
      if (mask) for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i & 3];
      this.buf = b.subarray(off + len);
      this.frame(op, fin, payload);
    }
  }

  frame(op, fin, payload) {
    if (op === 0x8) {
      const code = payload.length >= 2 ? payload.readUInt16BE(0) : 1005;
      const reason = payload.length > 2 ? payload.subarray(2).toString("utf8") : "";
      if (!this.closed) {
        try {
          this.write(0x8, payload.length >= 2 ? payload.subarray(0, 2) : Buffer.alloc(0));
        } catch {
          // gone
        }
      }
      this.finish({ code, reason });
      this.socket.end();
      return;
    }
    if (op === 0x9) return this.write(0xa, payload);
    if (op === 0xa) return;
    if (op === 0x1 || op === 0x2) this.frag = { text: op === 0x1, parts: [payload] };
    else if (op === 0x0 && this.frag) this.frag.parts.push(payload);
    if (fin && this.frag) {
      const m = { text: this.frag.text, data: Buffer.concat(this.frag.parts) };
      this.frag = null;
      const w = this.waiters.shift();
      if (w) w.resolve(m);
      else this.queue.push(m);
      this.emit("message", m);
    }
  }

  write(op, payload) {
    const data = Buffer.isBuffer(payload) ? payload : Buffer.from(payload);
    const len = data.length;
    const head = [0x80 | op];
    const maskBit = this.server ? 0 : 0x80;
    let ext = Buffer.alloc(0);
    if (len < 126) head.push(maskBit | len);
    else if (len < 65536) {
      head.push(maskBit | 126);
      ext = Buffer.alloc(2);
      ext.writeUInt16BE(len);
    } else {
      head.push(maskBit | 127);
      ext = Buffer.alloc(8);
      ext.writeBigUInt64BE(BigInt(len));
    }
    let body = data;
    let mask = Buffer.alloc(0);
    if (!this.server) {
      mask = crypto.randomBytes(4);
      body = Buffer.from(data);
      for (let i = 0; i < body.length; i++) body[i] ^= mask[i & 3];
    }
    this.socket.write(Buffer.concat([Buffer.from(head), ext, mask, body]));
  }

  sendText(s) {
    this.write(0x1, Buffer.from(s, "utf8"));
  }

  sendBinary(b) {
    this.write(0x2, b);
  }

  next(timeout = 15_000) {
    const m = this.queue.shift();
    if (m) return Promise.resolve(m);
    if (this.closed) return Promise.reject(new Error(`websocket closed (${this.closed.code} ${this.closed.reason})`));
    return new Promise((resolve, reject) => {
      const w = { resolve: (v) => (clearTimeout(t), resolve(v)), reject: (e) => (clearTimeout(t), reject(e)) };
      const t = setTimeout(() => {
        this.waiters.splice(this.waiters.indexOf(w), 1);
        reject(new Error("timed out waiting for a websocket message"));
      }, timeout);
      this.waiters.push(w);
    });
  }

  close(code = 1000) {
    if (this.closed) return;
    const p = Buffer.alloc(2);
    p.writeUInt16BE(code);
    try {
      this.write(0x8, p);
    } catch {
      // gone
    }
    setTimeout(() => this.socket.destroy(), 200);
  }
}

/** Client handshake through the relay (Host header picks the route). */
function wsConnect({ host, path: p, headers = {}, protocols = [], port = RELAY_PORT }) {
  return new Promise((resolve, reject) => {
    const key = crypto.randomBytes(16).toString("base64");
    const h = { Host: host, Connection: "Upgrade", Upgrade: "websocket", "Sec-WebSocket-Version": "13", "Sec-WebSocket-Key": key, ...headers };
    if (protocols.length) h["Sec-WebSocket-Protocol"] = protocols.join(", ");
    const req = http.request({ host: "127.0.0.1", port, path: p, headers: h, agent: false });
    req.on("upgrade", (res, socket, head) => {
      const expected = crypto.createHash("sha1").update(key + WS_GUID).digest("base64");
      if (res.headers["sec-websocket-accept"] !== expected) return reject(new Error("bad Sec-WebSocket-Accept"));
      resolve(new WsConn(socket, head, { protocol: res.headers["sec-websocket-protocol"] ?? null }));
    });
    req.on("response", (res) => {
      const chunks = [];
      res.on("data", (c) => chunks.push(c));
      res.on("end", () => reject(Object.assign(new Error(`upgrade refused: HTTP ${res.statusCode} ${Buffer.concat(chunks).toString()}`), { status: res.statusCode })));
    });
    req.on("error", reject);
    req.end();
  });
}

/** Server side of a WebSocket upgrade (for the local echo server behind the tunnel). */
function wsAccept(req, socket, head) {
  const key = req.headers["sec-websocket-key"];
  const accept = crypto.createHash("sha1").update(key + WS_GUID).digest("base64");
  const proto = (req.headers["sec-websocket-protocol"] ?? "").split(",")[0].trim();
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n${proto ? `Sec-WebSocket-Protocol: ${proto}\r\n` : ""}\r\n`);
  return new WsConn(socket, head, { server: true, protocol: proto || null });
}

// Relay protocol frames: [u8 type][u32 stream BE][payload] (relay/PROTOCOL.md §2).
const F = { HELLO: 0x01, INBOX_EVENT: 0x30, INBOX_ACK: 0x31, INBOX_STATUS: 0x32 };

function frame(type, stream, payload) {
  const p = Buffer.isBuffer(payload) ? payload : Buffer.from(JSON.stringify(payload));
  const h = Buffer.alloc(5);
  h[0] = type;
  h.writeUInt32BE(stream, 1);
  return Buffer.concat([h, p]);
}

function unframe(buf) {
  return { type: buf[0], stream: buf.readUInt32BE(1), payload: buf.subarray(5), json: () => JSON.parse(buf.subarray(5).toString("utf8")) };
}

/** A minimal relay inbox client: connect, collect frames. */
async function inboxClient(project) {
  const ws = await wsConnect({ host: relayHost, path: `/inbox?project=${project}`, headers: { Authorization: `Bearer ${TOKEN}` }, protocols: ["devall-relay.1"] });
  eq(ws.protocol, "devall-relay.1", "negotiated relay protocol");
  const frames = [];
  const waiters = [];
  ws.on("message", (m) => {
    if (m.text) return;
    const f = unframe(m.data);
    frames.push(f);
    for (const w of [...waiters]) if (w.check()) waiters.splice(waiters.indexOf(w), 1);
  });
  const waitFor = (pred, timeout = 15_000, what = "frame") =>
    new Promise((resolve, reject) => {
      const w = {
        check: () => {
          const i = frames.findIndex(pred);
          if (i < 0) return false;
          clearTimeout(t);
          resolve(frames.splice(i, 1)[0]);
          return true;
        },
      };
      const t = setTimeout(() => {
        waiters.splice(waiters.indexOf(w), 1);
        reject(new Error(`timed out waiting for ${what}`));
      }, timeout);
      if (!w.check()) waiters.push(w);
    });
  const hello = (await waitFor((f) => f.type === F.HELLO, 15_000, "HELLO")).json();
  return { ws, frames, hello, waitFor, events: (n, timeout) => collectEvents(waitFor, n, timeout), close: () => ws.close() };
}

async function collectEvents(waitFor, n, timeout = 15_000) {
  const out = [];
  for (let i = 0; i < n; i++) out.push((await waitFor((f) => f.type === F.INBOX_EVENT, timeout, `INBOX_EVENT #${i + 1}`)).json());
  return out;
}

// ---------- the local app behind the tunnels ----------

const app = { healthy: true, loadToken: null, alerts: [], loadHits: 0, hits: 0, healthAuth: [] };
const BIG_CHUNK = crypto.randomBytes(64 * 1024);
const GZIP_SOURCE = Buffer.from("compressed by the local app, never re-encoded by the relay\n".repeat(200));
const GZIPPED = zlib.gzipSync(GZIP_SOURCE);

function startApp() {
  return new Promise((resolve) => {
    const srv = http.createServer((req, res) => {
      app.hits++;
      const u = new URL(req.url, "http://local");
      const chunks = [];
      req.on("data", (c) => chunks.push(c));
      req.on("end", async () => {
        const body = Buffer.concat(chunks);
        if (req.headers["x-devall-load"]) app.loadHits++;
        if (u.pathname === "/hello") {
          res.writeHead(200, { "content-type": "application/json", "x-e2e": "yes" });
          return res.end(JSON.stringify({ method: req.method, url: req.url, headers: req.headers }));
        }
        if (u.pathname === "/echo") {
          res.writeHead(200, { "content-type": "application/octet-stream", "x-body-sha256": sha256(body) });
          return res.end(body);
        }
        if (u.pathname === "/big") {
          const n = Number(u.searchParams.get("chunks") ?? "16");
          res.writeHead(200, { "content-type": "application/octet-stream" });
          for (let i = 0; i < n; i++) {
            if (!res.write(BIG_CHUNK)) await new Promise((r) => res.once("drain", r));
          }
          return res.end();
        }
        if (u.pathname === "/sse") {
          res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
          for (let i = 1; i <= 3; i++) {
            res.write(`data: tick ${i}\n\n`);
            await sleep(400);
          }
          return res.end();
        }
        if (u.pathname === "/gzip") {
          res.writeHead(200, { "content-type": "text/plain", "content-encoding": "gzip" });
          return res.end(GZIPPED);
        }
        if (u.pathname === "/cookies") {
          res.writeHead(200, [
            ["set-cookie", "a=1; Path=/"],
            ["set-cookie", "b=2; Path=/; HttpOnly"],
            ["content-type", "text/plain"],
          ]);
          return res.end("cookies");
        }
        if (u.pathname === "/empty") {
          res.writeHead(204, { "x-empty": "1" });
          return res.end();
        }
        if (u.pathname === "/redirect") {
          res.writeHead(302, { location: "/hello?from=redirect" });
          return res.end();
        }
        if (u.pathname === "/health") {
          app.healthAuth.push(req.headers.authorization ?? null);
          res.writeHead(app.healthy ? 200 : 503, { "content-type": "application/json" });
          return res.end(JSON.stringify({ status: app.healthy ? "ok" : "down" }));
        }
        if (u.pathname === "/alert") {
          app.alerts.push(JSON.parse(body.toString("utf8") || "{}"));
          res.writeHead(204);
          return res.end();
        }
        if (u.pathname === "/.well-known/devall-load-verification.txt") {
          if (!app.loadToken) {
            res.writeHead(404);
            return res.end("no token");
          }
          res.writeHead(200, { "content-type": "text/plain" });
          return res.end(`${app.loadToken}\n`);
        }
        res.writeHead(404, { "content-type": "text/plain" });
        res.end("not found");
      });
    });
    srv.on("upgrade", (req, socket, head) => {
      if (!req.url.startsWith("/ws")) return socket.end("HTTP/1.1 404 Not Found\r\n\r\n");
      const ws = wsAccept(req, socket, head);
      ws.on("message", (m) => ws.write(m.text ? 0x1 : 0x2, m.data));
    });
    const sockets = new Set();
    srv.on("connection", (s) => {
      sockets.add(s);
      s.on("close", () => sockets.delete(s));
    });
    srv.listen(0, "127.0.0.1", () => resolve({ srv, port: srv.address().port, close: () => new Promise((r) => (srv.close(() => r()), sockets.forEach((s) => s.destroy()))) }));
  });
}

const TCP_GREETING = Buffer.from("220 e2e echo ready\r\n");

function startEcho() {
  return new Promise((resolve) => {
    const sockets = new Set();
    const srv = net.createServer((s) => {
      sockets.add(s);
      s.on("close", () => sockets.delete(s));
      s.on("error", () => {});
      // Speak first, like SMTP or MySQL servers: the relay must hold this greeting until the
      // consumer's upgrade completes.
      s.write(TCP_GREETING);
      s.pipe(s);
    });
    srv.listen(0, "127.0.0.1", () => resolve({ port: srv.address().port, close: () => new Promise((r) => (srv.close(() => r()), sockets.forEach((s) => s.destroy()))) }));
  });
}

// ---------- the relay under wrangler dev ----------

function wranglerConfig(tmp) {
  let t = fs.readFileSync(path.join(WORKER, "wrangler.toml"), "utf8");
  // No routes (they would make wrangler rewrite every URL to the zone) and an absolute entry point.
  t = t.replace(/^routes = \[[\s\S]*?^\]\s*$/m, "");
  t = t.replace(/^main = .*$/m, `main = "${path.join(WORKER, "src", "index.ts").replace(/\\/g, "/")}"`);
  t = t.replace(/^name = .*$/m, 'name = "devall-relay-e2e"');
  const file = path.join(tmp, "wrangler.e2e.toml");
  fs.writeFileSync(file, t);
  return file;
}

const MONITOR_KEY = crypto.randomBytes(32).toString("base64");

async function startRelay(tmp, port = null) {
  RELAY_PORT = port ?? (await freePort());
  const vars = {
    PUBLIC_DOMAIN: DOMAIN,
    AUTH_MODE: "static",
    RELAY_TOKENS: `e2e:${TOKEN},other:${OTHER_TOKEN},free:${FREE_TOKEN}`,
    // One token on the free tier (community-relay style) to exercise free limits for real.
    STATIC_PLANS: JSON.stringify({ free: "free" }),
    LIMITS_JSON: JSON.stringify({ free: { sessionSeconds: FREE_SESSION_SECONDS } }),
    ADMIN_TOKEN: ADMIN,
    MONITOR_SECRET_KEY: MONITOR_KEY,
    MOCK_URL_STYLE: "path",
    CUSTOM_HOSTS: "api.customer.test=e2e-http",
    LOAD_COLO_PROBE: "off",
    LOAD_VERIFY_DNS: "off",
    SOURCE_URL: "https://example.com/dev-all/relay",
  };
  const args = [
    path.join(WORKER, "node_modules", "wrangler", "bin", "wrangler.js"),
    "dev",
    "-c",
    wranglerConfig(tmp),
    "--local",
    "--ip",
    "127.0.0.1",
    "--port",
    String(RELAY_PORT),
    "--persist-to",
    path.join(tmp, "state"),
    "--test-scheduled",
    "--show-interactive-dev-session=false",
  ];
  for (const [k, v] of Object.entries(vars)) args.push("--var", `${k}:${v}`);
  const rec = startProc("wrangler", process.execPath, args, {
    cwd: tmp,
    log: path.join(tmp, port ? "wrangler-restarted.log" : "wrangler.log"),
    env: { WRANGLER_SEND_METRICS: "false", WRANGLER_SEND_ERROR_REPORTS: "false", CLOUDFLARE_CF_FETCH_ENABLED: "false", NO_COLOR: "1", CI: "1" },
  });
  ctx.relay = rec;
  const deadline = Date.now() + 90_000;
  for (;;) {
    if (rec.exited) throw new Error(`wrangler exited: ${rec.errLines.slice(-20).join("\n")}\n${rec.lines.slice(-20).join("\n")}`);
    try {
      const r = await relayReq({ host: relayHost, path: "/healthz" });
      if (r.status === 200) break;
    } catch {
      // not up yet
    }
    if (Date.now() > deadline) throw new Error("wrangler dev didn't come up within 90 s");
    await sleep(500);
  }
  return rec;
}

// ---------- scenarios ----------

const ctx = {};

const cliEnv = () => ({ DEVALL_RELAY_URL: `ws://127.0.0.1:${RELAY_PORT}`, DEVALL_RELAY_TOKEN: TOKEN, RUST_BACKTRACE: "1" });

async function scenarioTunnel() {
  const t = startProc("devall tunnel http", CLI, ["tunnel", "http", "--port", String(ctx.app.port), "--subdomain", "e2e-http", "--json"], { env: cliEnv() });
  ctx.tunnel = t;
  const { value: hello } = await waitLine(t, jsonLine((v) => v.type === "connected"), { what: "the connected line" });
  ctx.tunnelSince = Date.now();
  eq(hello.publicUrl, `https://e2e-http.${DOMAIN}`, "public URL");
  ctx.publicUrls = hello.publicUrls;
  const host = `e2e-http.${DOMAIN}`;

  // Status, body, headers (incl. X-Forwarded-*) and the public Host kept.
  const r = await relayReq({ host, path: "/hello?x=1&y=%20z", headers: { "X-Custom": "abc" } });
  eq(r.status, 200, "GET /hello status");
  eq(r.headers["x-e2e"], "yes", "local response header");
  assert(String(r.headers["x-robots-tag"]).includes("noindex"), "relay adds x-robots-tag");
  const seen = JSON.parse(r.body.toString());
  eq(seen.url, "/hello?x=1&y=%20z", "path + query verbatim");
  eq(seen.headers.host, host, "public Host kept");
  eq(seen.headers["x-custom"], "abc", "request header forwarded");
  eq(seen.headers["x-forwarded-host"], host, "X-Forwarded-Host");
  eq(seen.headers["x-forwarded-proto"], "http", "X-Forwarded-Proto (wrangler dev serves http)");
  eq(seen.headers["x-forwarded-port"], "80", "X-Forwarded-Port");
  eq(seen.headers["x-forwarded-for"], "127.0.0.1", "X-Forwarded-For");
  assert(!Object.keys(seen.headers).some((k) => k.startsWith("x-devall-") || k.startsWith("cf-")), "no relay/edge headers leak to the app");

  // Request body byte-for-byte (larger than one frame and the initial window).
  const payload = crypto.randomBytes(300 * 1024);
  const echo = await relayReq({ method: "POST", host, path: "/echo", body: payload, headers: { "Content-Type": "application/octet-stream" } });
  eq(echo.status, 200, "POST /echo status");
  eq(echo.headers["x-body-sha256"], sha256(payload), "request body reached the app intact");
  eq(sha256(echo.body), sha256(payload), "response body intact");

  // A streamed large response (8 MiB, chunked) — exercises per-stream credit.
  const chunks = 128;
  const big = await relayReq({ host, path: `/big?chunks=${chunks}` });
  eq(big.status, 200, "GET /big status");
  eq(big.body.length, chunks * BIG_CHUNK.length, "streamed body length");
  const h = crypto.createHash("sha256");
  for (let i = 0; i < chunks; i++) h.update(BIG_CHUNK);
  eq(sha256(big.body), h.digest("hex"), "streamed body content");

  // A chunked upload (no Content-Length) arrives intact too.
  const chunked = await new Promise((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port: RELAY_PORT, method: "POST", path: "/echo", headers: { Host: host, "Transfer-Encoding": "chunked" }, agent: false }, (res) => {
      const parts = [];
      res.on("data", (c) => parts.push(c));
      res.on("end", () => resolve({ status: res.statusCode, sha: res.headers["x-body-sha256"], body: Buffer.concat(parts) }));
    });
    req.on("error", reject);
    for (let i = 0; i < 10; i++) req.write(BIG_CHUNK);
    req.end();
  });
  const sent = crypto.createHash("sha256");
  for (let i = 0; i < 10; i++) sent.update(BIG_CHUNK);
  eq([chunked.status, chunked.sha], [200, sent.digest("hex")], "chunked upload reached the app intact");

  // Server-sent events stream incrementally (the first event long before the last).
  const sse = await new Promise((resolve, reject) => {
    const t0 = Date.now();
    const times = [];
    const req = http.request({ host: "127.0.0.1", port: RELAY_PORT, path: "/sse", headers: { Host: host, Accept: "text/event-stream" }, agent: false }, (res) => {
      let text = "";
      res.on("data", (c) => {
        text += c;
        times.push(Date.now() - t0);
      });
      res.on("end", () => resolve({ status: res.statusCode, type: res.headers["content-type"], text, first: times[0], last: Date.now() - t0 }));
    });
    req.on("error", reject);
    req.end();
  });
  eq([sse.status, sse.type], [200, "text/event-stream"], "SSE response");
  eq(sse.text, "data: tick 1\n\ndata: tick 2\n\ndata: tick 3\n\n", "SSE body");
  assert(sse.last - sse.first >= 600, `SSE events arrive as they are sent (first at ${sse.first} ms, end at ${sse.last} ms)`);

  // Content-Encoding passes through untouched; several Set-Cookie headers survive; 204/HEAD/302.
  const gz = await relayReq({ host, path: "/gzip", headers: { "Accept-Encoding": "gzip" } });
  eq([gz.status, gz.headers["content-encoding"], sha256(gz.body)], [200, "gzip", sha256(GZIPPED)], "gzip body byte-for-byte");
  const ck = await relayReq({ host, path: "/cookies" });
  eq(ck.headers["set-cookie"], ["a=1; Path=/", "b=2; Path=/; HttpOnly"], "multiple Set-Cookie headers");
  const empty = await relayReq({ host, path: "/empty" });
  eq([empty.status, empty.headers["x-empty"], empty.body.length], [204, "1", 0], "204 without a body");
  const head = await relayReq({ method: "HEAD", host, path: "/hello" });
  eq([head.status, head.body.length], [200, 0], "HEAD");
  const redirect = await relayReq({ host, path: "/redirect" });
  eq([redirect.status, redirect.headers.location], [302, "/hello?from=redirect"], "redirects are passed through, not followed");
  const missing = await relayReq({ host, path: "/missing" });
  eq(missing.status, 404, "the app's 404 passes through");

  // 30 concurrent requests share the one tunnel socket.
  const many = await Promise.all(Array.from({ length: 30 }, (_, i) => relayReq({ method: "POST", host, path: `/echo?i=${i}`, body: `body-${i}` })));
  assert(many.every((r, i) => r.status === 200 && r.body.toString() === `body-${i}`), "30 concurrent requests answered correctly");

  // WebSocket pass-through: subprotocol, text and binary echo, close.
  const ws = await wsConnect({ host, path: "/ws?room=1", protocols: ["e2e.v1", "e2e.v0"] });
  eq(ws.protocol, "e2e.v1", "negotiated subprotocol from the local server");
  ws.sendText("ping-1");
  const m1 = await ws.next();
  eq([m1.text, m1.data.toString()], [true, "ping-1"], "text echo");
  const bin = crypto.randomBytes(100 * 1024);
  ws.sendBinary(bin);
  const m2 = await ws.next();
  eq([m2.text, sha256(m2.data)], [false, sha256(bin)], "binary echo (100 KiB)");
  ws.close(1000);
  await sleep(300);

  // The CLI's request log.
  await waitLine(t, jsonLine((v) => v.type === "request" && v.log.websocket), { what: "the websocket log line" });
  const logs = t.lines.map((l) => jsonLine((v) => v.type === "request")(l)).filter(Boolean).map((v) => v.log);
  for (const p of ["/hello?x=1&y=%20z", "/echo", `/big?chunks=${chunks}`, "/sse"]) assert(logs.some((l) => l.path === p && l.status === 200), `request log has ${p}`);
  const bigLog = logs.find((l) => l.path.startsWith("/big"));
  eq(bigLog.bytesOut, chunks * BIG_CHUNK.length, "request log counts response bytes");
  return `URL ${hello.publicUrl}; 8 MiB stream, 300 KiB + chunked uploads, SSE, gzip, cookies, 204/HEAD/302, 30 concurrent, WS echo, ${logs.length} log lines`;
}

async function scenarioTcp() {
  const echo = await startEcho();
  ctx.cleanup.push(echo.close);
  const pub = startProc("devall tunnel tcp", CLI, ["tunnel", "tcp", "--port", String(echo.port), "--name", "e2e-tcp"], { env: cliEnv() });
  const { value: secretLine } = await waitLine(pub, (l) => /--secret (dtcp_\w+)/.exec(l)?.[1] ?? false, { stream: "err", what: "the connect command with the access secret" });
  const secret = secretLine;
  const ownerPort = await freePort();
  const secretPort = await freePort();
  const owner = startProc("devall tcp connect (owner)", CLI, ["tcp", "connect", "e2e-tcp", "--local-port", String(ownerPort)], { env: cliEnv() });
  const viaSecret = startProc("devall tcp connect (secret)", CLI, ["tcp", "connect", "e2e-tcp", "--local-port", String(secretPort), "--secret", secret], { env: { DEVALL_RELAY_URL: `ws://127.0.0.1:${RELAY_PORT}`, DEVALL_RELAY_TOKEN: "" } });
  await waitLine(owner, (l) => l.includes("Forwarding"), { stream: "err", what: "owner consumer listening" });
  await waitLine(viaSecret, (l) => l.includes("Forwarding"), { stream: "err", what: "secret consumer listening" });

  const roundTrip = (port, data) =>
    new Promise((resolve, reject) => {
      const s = net.connect(port, "127.0.0.1");
      const got = [];
      let n = 0;
      const want = TCP_GREETING.length + data.length;
      let sent = false;
      s.on("data", (d) => {
        got.push(d);
        n += d.length;
        // The server's greeting comes first (through the relay); then send.
        if (!sent && n >= TCP_GREETING.length) {
          sent = true;
          s.write(data);
        }
        if (n >= want) {
          s.end();
          resolve(Buffer.concat(got));
        }
      });
      s.on("error", reject);
      s.setTimeout(60_000, () => s.destroy(new Error(`tcp round trip timed out after ${n} bytes`)));
    });
  const big = crypto.randomBytes(4 * 1024 * 1024);
  const [a, b] = await Promise.all([roundTrip(ownerPort, big), roundTrip(secretPort, Buffer.from("hello over a secret"))]);
  eq(a.subarray(0, TCP_GREETING.length).toString(), TCP_GREETING.toString(), "server greeting arrives first");
  eq(sha256(a.subarray(TCP_GREETING.length)), sha256(big), "4 MiB echoed byte-for-byte through the TCP tunnel");
  eq(b.subarray(TCP_GREETING.length).toString(), "hello over a secret", "consumer with the access secret");
  await waitLine(pub, (l) => l.includes("connection from"), { stream: "err", what: "the publisher's connection log" });
  // A wrong secret is refused by the relay: no bytes come back and the consumer says why.
  const badPort = await freePort();
  const bad = startProc("devall tcp connect (wrong secret)", CLI, ["tcp", "connect", "e2e-tcp", "--local-port", String(badPort), "--secret", "dtcp_wrongwrongwrongwrongwrongwrongwrongwrong"], { env: { DEVALL_RELAY_URL: `ws://127.0.0.1:${RELAY_PORT}`, DEVALL_RELAY_TOKEN: "" } });
  await waitLine(bad, (l) => l.includes("Forwarding"), { stream: "err", what: "wrong-secret consumer listening" });
  const refused = await new Promise((resolve) => {
    const sock = net.connect(badPort, "127.0.0.1");
    let n = 0;
    sock.on("data", (d) => (n += d.length));
    sock.on("close", () => resolve(n));
    sock.on("error", () => resolve(n));
    sock.setTimeout(20_000, () => sock.destroy());
  });
  eq(refused, 0, "no bytes through a wrong secret");
  await waitLine(bad, (l) => /access secret|forbidden/i.test(l), { stream: "err", what: "the relay's refusal reason in the consumer log" });
  for (const p of [owner, viaSecret, bad, pub]) await stopProc(p);
  return "server greeting + 4 MiB echo (owner account) + access-secret consumer; wrong secret refused with reason";
}

async function scenarioInbox() {
  const key = "e2e-inbox";
  // First connection allocates the inbox; then nobody is connected.
  let c = await inboxClient(key);
  const inboxId = c.hello.inboxId;
  assert(/^[a-z0-9]{20}$/.test(inboxId), "inbox id");
  eq(c.hello.publicUrl, `https://hooks.${DOMAIN}/${inboxId}/`, "inbox URL");
  c.close();
  await sleep(500);

  const bodies = [
    { path: "/stripe?n=1", type: "application/json", body: Buffer.from(JSON.stringify({ id: "evt_1", amount: 42, note: "héllo ✓" })) },
    { path: "/bytes?n=2", type: "application/octet-stream", body: Buffer.concat([Buffer.from([...Array(256).keys()]), crypto.randomBytes(2000)]) },
    { path: "/form?n=3", type: "application/x-www-form-urlencoded", body: Buffer.from("a=1&b=%E2%9C%93&c=") },
  ];
  const ids = [];
  for (const b of bodies) {
    const r = await relayReq({ method: "POST", host: `hooks.${DOMAIN}`, path: `/${inboxId}${b.path}`, body: b.body, headers: { "Content-Type": b.type, "Stripe-Signature": "t=1,v1=abc" } });
    eq(r.status, 200, `webhook ${b.path} accepted while offline`);
    eq(r.body.toString(), '{"ok":true}', "default inbox answer");
    ids.push(r.headers["x-devall-event-id"]);
  }

  // Buffered events arrive in order, byte-for-byte; acknowledge only the first.
  c = await inboxClient(key);
  let evs = await c.events(3);
  eq(evs.map((e) => e.id), ids, "buffered events in receive order");
  evs.forEach((e, i) => {
    eq(e.path, bodies[i].path, `event ${i + 1} path`);
    eq(e.url, `http://hooks.${DOMAIN}/${inboxId}${bodies[i].path}`, `event ${i + 1} exact URL`);
    eq(sha256(Buffer.from(e.bodyBase64, "base64")), sha256(bodies[i].body), `event ${i + 1} raw body`);
    assert(e.headers.some(([k, v]) => k === "stripe-signature" && v === "t=1,v1=abc"), "signature header kept");
  });
  c.ws.sendBinary(frame(F.INBOX_ACK, 0, { ids: [ids[0]] }));
  const st = (await c.waitFor((f) => f.type === F.INBOX_STATUS && f.json().buffered === 2, 10_000, "status after the ack")).json();
  eq(st.buffered, 2, "two events still buffered");
  c.close();
  await sleep(500);

  // Reconnect: the unacknowledged events come again (at-least-once), the acknowledged one doesn't.
  c = await inboxClient(key);
  evs = await c.events(2);
  eq(evs.map((e) => e.id), ids.slice(1), "redelivery after reconnect = exactly the unacknowledged events");
  await sleep(700);
  assert(!c.frames.some((f) => f.type === F.INBOX_EVENT), "nothing else redelivered");
  c.close();
  await sleep(500);

  // The CLI takes them, stores them in the project's webhook store, acknowledges.
  const proj = ctx.inboxProject;
  const listen = startProc("devall inbox listen", CLI, ["inbox", "listen", "--project", proj, "--key", key, "--json"], { env: cliEnv() });
  await waitLine(listen, jsonLine((v) => v.type === "connected" && v.inboxId === inboxId), { what: "inbox connected" });
  const got = [];
  for (const id of ids.slice(1)) {
    const { value } = await waitLine(listen, jsonLine((v) => v.type === "event" && v.id === id), { what: `event ${id}` });
    got.push(value);
  }
  got.forEach((e, i) => {
    assert(e.stored, "stored in the project's webhook store");
    eq(sha256(Buffer.from(e.bodyBase64, "base64")), sha256(bodies[i + 1].body), "CLI received the raw body");
  });
  await waitLine(listen, jsonLine((v) => v.type === "status" && v.buffered === 0 && v.delivered === 2), { what: "buffer drained" });
  // A live event while connected.
  const live = await relayReq({ method: "POST", host: `hooks.${DOMAIN}`, path: `/${inboxId}/live`, body: Buffer.from("live!"), headers: { "Content-Type": "text/plain" } });
  const liveId = live.headers["x-devall-event-id"];
  await waitLine(listen, jsonLine((v) => v.type === "event" && v.id === liveId && Buffer.from(v.bodyBase64, "base64").toString() === "live!"), { what: "the live event" });
  await waitLine(listen, jsonLine((v) => v.type === "status" && v.buffered === 0 && v.delivered === 3), { what: "live event acknowledged" });
  await stopProc(listen, true);

  // Reconnect again: everything was acknowledged, nothing comes back (no duplicates).
  const again = startProc("devall inbox listen (again)", CLI, ["inbox", "listen", "--project", proj, "--key", key, "--json"], { env: cliEnv() });
  await waitLine(again, jsonLine((v) => v.type === "connected"), { what: "inbox connected again" });
  await sleep(2500);
  assert(!again.lines.some((l) => l.includes('"type":"event"')), "no event delivered twice");
  await stopProc(again);
  assert(fs.existsSync(path.join(proj, ".devall", "webhooks.sqlite")), "webhook store written");
  return `3 buffered (ordered, byte-exact) + 1 live; ack/redelivery/no-dup checked`;
}

const PETS = {
  mappings: [
    { name: "list pets", request: { method: "GET", urlPath: "/pets" }, response: { status: 200, headers: { "Content-Type": "application/json" }, jsonBody: [{ id: 1, name: "Rex" }] } },
    {
      name: "one pet",
      request: { method: "GET", urlPathTemplate: "/pets/{id}" },
      response: { status: 200, headers: { "Content-Type": "application/json", "X-Pet": "{{request.path.id}}" }, body: '{"id":{{request.path.id}},"q":"{{request.query.q}}","n":{{randomInt lower=5 upper=5}}}', transformers: ["response-template"] },
    },
    { name: "order pending", scenarioName: "order", requiredScenarioState: "Started", request: { method: "GET", urlPath: "/order" }, response: { status: 200, body: "pending" } },
    { name: "pay", scenarioName: "order", requiredScenarioState: "Started", newScenarioState: "Paid", request: { method: "POST", urlPath: "/order/pay" }, response: { status: 204 } },
    { name: "order paid", scenarioName: "order", requiredScenarioState: "Paid", request: { method: "GET", urlPath: "/order" }, response: { status: 200, body: "paid" } },
    { name: "file body", request: { method: "GET", urlPath: "/catalog" }, response: { status: 200, headers: { "Content-Type": "application/json" }, bodyFileName: "catalog.json" } },
  ],
};
const CATALOG = JSON.stringify({ items: ["a", "b", "ü"], from: "__files" });

async function scenarioMock() {
  const proj = ctx.mockProject;
  fs.mkdirSync(path.join(proj, "mocks", "mappings"), { recursive: true });
  fs.writeFileSync(path.join(proj, "mocks", "mappings", "pets.json"), JSON.stringify(PETS, null, 2));
  fs.mkdirSync(path.join(proj, "mocks", "__files"), { recursive: true });
  fs.writeFileSync(path.join(proj, "mocks", "__files", "catalog.json"), CATALOG);
  const pub = await runCli(["mock", "publish", "--project", proj, "--name", "e2e-pets"], cliEnv());
  eq(pub.code, 0, `mock publish exit code (${pub.stderr})`);
  const m = (method, p, body = null) => relayReq({ method, host: `mock.${DOMAIN}`, path: `/e2e-pets${p}`, body });
  const list = await m("GET", "/pets");
  eq([list.status, JSON.parse(list.body.toString())], [200, [{ id: 1, name: "Rex" }]], "list pets");
  const one = await m("GET", "/pets/42?q=hi");
  eq(one.status, 200, "templated stub status");
  eq(one.headers["x-pet"], "42", "templated header");
  eq(JSON.parse(one.body.toString()), { id: 42, q: "hi", n: 5 }, "templated body");
  eq((await m("GET", "/order")).body.toString(), "pending", "scenario starts in Started");
  eq((await m("POST", "/order/pay", "")).status, 204, "scenario transition");
  eq((await m("GET", "/order")).body.toString(), "paid", "scenario moved to Paid");
  eq((await m("GET", "/nope")).status, 404, "unmatched request");
  const file = await m("GET", "/catalog");
  eq([file.status, file.body.toString()], [200, CATALOG], "bodyFileName served from the uploaded __files");
  const j = await runCli(["mock", "journal", "e2e-pets", "--json", "--limit", "20"], cliEnv());
  eq(j.code, 0, `mock journal exit code (${j.stderr})`);
  const entries = j.lines.filter((l) => l.startsWith("{")).map((l) => JSON.parse(l));
  const flat = entries.length === 1 && Array.isArray(entries[0]) ? entries[0] : entries;
  for (const p of ["/pets", "/pets/42?q=hi", "/order/pay", "/nope"]) assert(flat.some((e) => String(e.url ?? "").endsWith(p) || String(e.url ?? "").includes(p)), `journal has ${p} (${j.stdout.slice(0, 300)})`);
  const un = await runCli(["mock", "unpublish", "e2e-pets"], cliEnv());
  eq(un.code, 0, `mock unpublish exit code (${un.stderr})`);
  eq((await m("GET", "/pets")).status, 404, "unpublished mock answers 404");
  return `6 stubs (templating, scenario, __files), ${flat.length} journal entries, unpublished`;
}

async function scenarioShare() {
  const secret = "sk_live_should_never_appear";
  // What the desktop uploads (History → Share…): already redacted on the client.
  const snapshot = {
    version: 1,
    title: "GET /users",
    request: { method: "GET", url: "https://api.example.com/users?page=2&api_key=••••••", headers: [["accept", "application/json"], ["authorization", "••••••"]] },
    response: { status: 200, statusText: "OK", headers: [["content-type", "application/json"]], body: '{"users":[{"id":1}]}', size: 20, contentType: "application/json" },
    timing: { totalMs: 123 },
    environment: "staging",
    at: Date.now(),
    curl: "curl 'https://api.example.com/users?page=2&api_key=••••••' -H 'authorization: ••••••'",
    redactions: ["secret values", "authorization, cookie and API-key headers"],
  };
  const created = await api("POST", "/v1/shares", { snapshot, ttlSeconds: 3600 });
  eq(created.status, 201, "share created");
  const { id, url } = created.body;
  eq(url, `https://share.${DOMAIN}/${id}`, "share URL");
  const page = await relayReq({ host: `share.${DOMAIN}`, path: `/${id}`, headers: { Accept: "text/html" } });
  eq(page.status, 200, "share page");
  const html = page.body.toString();
  assert(html.includes("api.example.com/users?page=2"), "page shows the request");
  assert(!html.includes(secret), "no secret on the page");
  assert(String(page.headers["content-security-policy"]).length > 0, "share page has a CSP");
  const raw = await relayReq({ host: `share.${DOMAIN}`, path: `/${id}/raw.json` });
  eq([raw.status, JSON.parse(raw.body.toString()).title], [200, "GET /users"], "raw.json download");
  const listed = await api("GET", "/v1/shares");
  assert(listed.body.shares.some((s) => s.id === id), "share listed");
  eq((await api("DELETE", `/v1/shares/${id}`)).status, 200, "revoke");
  const gone = await relayReq({ host: `share.${DOMAIN}`, path: `/${id}` });
  assert(gone.status === 410 || gone.status === 404, `revoked share answers 410/404 (got ${gone.status})`);
  return `created, viewed, revoked → ${gone.status}`;
}

async function pollResults(id, n, timeout = 20_000) {
  const end = Date.now() + timeout;
  for (;;) {
    const r = await api("GET", `/v1/monitors/${id}/results?limit=10`);
    if (r.status === 200 && r.body.results.length >= n) return r.body.results;
    if (Date.now() > end) throw new Error(`monitor ${id}: ${r.body?.results?.length ?? 0} results after ${timeout} ms (wanted ${n})`);
    await sleep(300);
  }
}

/**
 * Fires the cron for a given scheduled time. `wrangler dev --test-scheduled`'s `/__scheduled`
 * ignores `?time=` in this wrangler version (the Worker gets "now"), so this uses the Local
 * Explorer API wrangler dev exposes, which passes `scheduled_time` through.
 */
async function fireCron(timeMs) {
  const r = await relayReq({
    method: "POST",
    host: `127.0.0.1:${RELAY_PORT}`,
    path: "/cdn-cgi/local/explorer/api/local/scheduled?worker=devall-relay-e2e",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ cron: "*/5 * * * *", scheduled_time: timeMs }),
  });
  eq(r.status, 200, `scheduled handler ran (${r.body.toString().slice(0, 300)})`);
}

async function scenarioMonitor() {
  app.healthy = true;
  app.alerts.length = 0;
  const created = await api("POST", "/v1/monitors", {
    name: "local health",
    intervalMinutes: 5,
    requests: [
      {
        name: "health",
        method: "GET",
        url: `http://127.0.0.1:${ctx.app.port}/health`,
        headers: [["authorization", "Bearer {{secret.HEALTH_TOKEN}}"]],
        assertions: [{ assert: "status", op: "eq", value: "200" }, { assert: "jsonPath", path: "$.status", op: "eq", value: "ok" }],
      },
    ],
    alerts: { webhookUrl: `http://127.0.0.1:${ctx.app.port}/alert`, onRecovery: true },
    // Uploaded only when the user ticks it (the desktop); stored AES-GCM-encrypted.
    secrets: { HEALTH_TOKEN: "mon-secret-0123456789" },
  });
  eq(created.status, 201, `monitor created (${JSON.stringify(created.body)})`);
  eq(created.body.secretNames, ["HEALTH_TOKEN"], "secret stored by name");
  assert(!JSON.stringify(created.body).includes("mon-secret-0123456789"), "secret value never returned");
  const id = created.body.id;
  app.healthAuth.length = 0;
  const slot = Math.floor(Date.now() / 300_000) * 300_000; // a */5 slot
  await fireCron(slot);
  let results = await pollResults(id, 1);
  eq([results[0].ok, results[0].trigger, results[0].checks[0].status], [true, "schedule", 200], "first scheduled run passes");
  eq(app.healthAuth, ["Bearer mon-secret-0123456789"], "the secret was substituted only in the outgoing request");
  assert(!JSON.stringify(results).includes("mon-secret-0123456789"), "results never contain the secret");
  app.healthy = false;
  await fireCron(slot + 300_000);
  results = await pollResults(id, 2);
  eq([results[0].ok, results[0].checks[0].status], [false, 503], "second scheduled run records the failure");
  assert(results[0].checks[0].failures.join(" ").includes("status eq 200"), "failure message");
  const end = Date.now() + 10_000;
  while (!app.alerts.length && Date.now() < end) await sleep(200);
  eq(app.alerts.length, 1, "one alert webhook received");
  eq([app.alerts[0].status, app.alerts[0].monitor.id], ["down", id], "down alert payload");
  // Recovery alert.
  app.healthy = true;
  await fireCron(slot + 600_000);
  results = await pollResults(id, 3);
  eq(results[0].ok, true, "third run passes again");
  const end2 = Date.now() + 10_000;
  while (app.alerts.length < 2 && Date.now() < end2) await sleep(200);
  eq(app.alerts[1]?.status, "up", "recovery alert");
  const view = await api("GET", `/v1/monitors/${id}`);
  eq([view.body.runs, Math.round(view.body.uptime * 100)], [3, 67], "runs and uptime");
  eq((await api("DELETE", `/v1/monitors/${id}`)).status, 200, "monitor deleted");
  return "pass → fail (alert down) → pass (alert up), cron fired with explicit scheduled times";
}

async function scenarioCustomDomain() {
  if (!ctx.tunnel || ctx.tunnel.exited) throw new Error("the HTTP tunnel from scenario 1 isn't running");
  assert((ctx.publicUrls ?? []).includes("https://api.customer.test"), `HELLO lists the custom domain (${JSON.stringify(ctx.publicUrls)})`);
  const r = await relayReq({ host: "api.customer.test", path: "/hello?via=custom" });
  eq(r.status, 200, "custom host reaches the tunnel");
  const seen = JSON.parse(r.body.toString());
  eq(seen.headers["x-forwarded-host"], "api.customer.test", "X-Forwarded-Host is the custom domain");
  eq(seen.headers.host, "api.customer.test", "Host is the custom domain");
  const unknown = await relayReq({ host: "unknown.customer.test", path: "/" });
  eq(unknown.status, 404, "unmapped host");
  const domains = await api("GET", "/v1/domains");
  assert(domains.body.domains.some((d) => d.hostname === "api.customer.test" && d.status === "static"), "listed by /v1/domains");
  return "api.customer.test → e2e-http tunnel";
}

async function scenarioCloudLoad() {
  const origin = `http://127.0.0.1:${ctx.app.port}`;
  app.loadToken = null;
  const refused = await runCli(["load", "cloud", "verify", origin, "--json"], cliEnv());
  eq(refused.code, 1, `verification refused without the token file (${refused.stderr})`);
  const pending = JSON.parse(refused.lines.find((l) => l.startsWith("{")));
  eq(pending.status, "pending", "target pending");
  assert(/^devall-load-\w{26}$/.test(pending.token), "token");
  // No load before verification.
  const early = await api("POST", "/v1/load/runs", { name: "x", regions: ["weur"], profile: { startRate: 1, stages: [{ durationMs: 1000, target: 1 }] }, requests: [{ name: "h", path: "h", method: "GET", url: `${origin}/hello`, headers: [] }], weights: [1] });
  eq([early.status, early.body.error.code], [403, "target_not_verified"], "runs refused before verification");

  app.loadToken = pending.token;
  await sleep(2100);
  const ok = await runCli(["load", "cloud", "verify", origin, "--json"], cliEnv());
  eq(ok.code, 0, `verification passes once the file is served (${ok.stderr})`);
  eq(JSON.parse(ok.lines.find((l) => l.startsWith("{"))).status, "verified", "target verified");

  const proj = ctx.loadProject;
  fs.mkdirSync(path.join(proj, "http"), { recursive: true });
  fs.mkdirSync(path.join(proj, "load"), { recursive: true });
  fs.writeFileSync(path.join(proj, "http", "hello.req.yaml"), `version: 1\nid: 01JE2E0000000000000000HELL\nname: Hello\nmethod: GET\nurl: ${origin}/hello\nheaders:\n- name: accept\n  value: application/json\n`);
  fs.writeFileSync(
    path.join(proj, "load", "edge.load.yaml"),
    "version: 1\nname: Edge smoke\nscenario:\n  requests:\n  - path: hello.req.yaml\nexecutor:\n  type: constantArrivalRate\n  rate: 4\n  duration: 5s\n  preAllocatedVus: 4\nthresholds:\n- errorRate < 1%\n- p95 < 5s\n",
  );
  app.loadHits = 0;
  const run = await runCli(["load", "cloud", "run", "edge.load.yaml", "--project", proj, "--regions", "weur,enam", "--json", "--no-keychain"], cliEnv(), 180_000);
  eq(run.code, 0, `cloud run exit code (stderr: ${run.stderr})`);
  const s = JSON.parse(run.lines.find((l) => l.startsWith("{")));
  eq(s.totals.requests, 20, "4/s for 5 s = 20 requests in the merged summary");
  eq(s.totals.failed, 0, "no failures");
  eq(s.statusCodes, { 200: 20 }, "status codes");
  eq(s.openModel, true, "open model");
  eq(s.agents.map((a) => [a.address, a.requests]), [["cloud:weur", 10], ["cloud:enam", 10]], "per-region split");
  eq(s.thresholdsPassed, true, "thresholds evaluated on the merged result");
  eq(app.loadHits, 20, "the target saw 20 requests tagged x-devall-load");
  assert(/summary: load[\\/]runs[\\/]/.test(run.stderr), "summary written to load/runs/");
  // A ramp longer than one 10 s segment from three regions: 0 → 30 iterations/s over 12 s is
  // exactly 180 iterations (60 per region), none lost or repeated at the segment boundary.
  fs.writeFileSync(
    path.join(proj, "load", "ramp.load.yaml"),
    "version: 1\nname: Edge ramp\nscenario:\n  requests:\n  - path: hello.req.yaml\nexecutor:\n  type: rampingArrivalRate\n  startRate: 0\n  stages:\n  - duration: 12s\n    target: 30\n  preAllocatedVus: 10\n  maxVus: 60\n",
  );
  app.loadHits = 0;
  const ramp = await runCli(["load", "cloud", "run", "ramp.load.yaml", "--project", proj, "--regions", "weur,apac,oc", "--json", "--no-keychain", "--no-save"], cliEnv(), 180_000);
  eq(ramp.code, 0, `ramp exit code (stderr: ${ramp.stderr})`);
  const rs = JSON.parse(ramp.lines.find((l) => l.startsWith("{")));
  eq([rs.totals.requests, rs.totals.iterations, rs.totals.failed, rs.totals.droppedIterations], [180, 180, 0, 0], "ramp totals");
  eq(rs.agents.map((a) => a.requests), [60, 60, 60], "ramp split across three regions");
  eq(app.loadHits, 180, "the target saw every ramp request");
  assert(rs.timeline.length >= 12, `timeline buckets (${rs.timeline.length})`);
  // Kill switch.
  eq((await relayReq({ method: "POST", host: relayHost, path: "/admin/load/disable", headers: { Authorization: `Bearer ${ADMIN}` } })).status, 200, "admin disable");
  const off = await runCli(["load", "cloud", "run", "edge.load.yaml", "--project", proj, "--regions", "weur", "--no-save", "--no-keychain"], cliEnv());
  eq(off.code, 1, "refused while disabled");
  assert(off.stderr.includes("load_disabled"), `kill switch message (${off.stderr})`);
  eq((await relayReq({ method: "POST", host: relayHost, path: "/admin/load/enable", headers: { Authorization: `Bearer ${ADMIN}` } })).status, 200, "admin enable");
  return `verify refused → verified; 2 regions × 10 requests merged (p95 ${s.latency.p95} ms); 3-region 12 s ramp = 180 exactly; kill switch`;
}


const BROWSER = { Accept: "text/html,application/xhtml+xml", "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/141.0", "Sec-Fetch-Dest": "document", "Sec-Fetch-Mode": "navigate" };

async function scenarioFreeTier() {
  const env = { ...cliEnv(), DEVALL_RELAY_TOKEN: FREE_TOKEN };
  const t = startProc("devall tunnel http (free)", CLI, ["tunnel", "http", "--port", String(ctx.app.port), "--json"], { env });
  const { value: hello } = await waitLine(t, jsonLine((v) => v.type === "connected"), { what: "free tunnel connected" });
  eq(hello.plan, "free", "free plan");
  const sub = hello.subdomain;
  assert(/^[a-z]+-[a-z]+-\d{4}$/.test(sub), `random adjective-noun-NNNN name (${sub})`);
  const host = `${sub}.${DOMAIN}`;
  // API clients pass straight through; browsers get the warning page once.
  const api_ = await relayReq({ host, path: "/hello" });
  eq(api_.status, 200, "API request through a free tunnel");
  const hitsBefore = app.hits;
  const page = await relayReq({ host, path: "/", headers: BROWSER });
  eq(page.status, 200, "interstitial page");
  assert(page.body.toString().includes(host) && /report/i.test(page.body.toString()), "interstitial names the host and links abuse reports");
  eq(app.hits, hitsBefore, "the browser navigation didn't reach the app");
  const skip = await relayReq({ host, path: "/hello", headers: { ...BROWSER, "devall-skip-browser-warning": "1" } });
  eq(skip.status, 200, "skip header bypasses the page");
  // One tunnel at a time on the free tier.
  const second = await runCli(["tunnel", "http", "--port", String(ctx.app.port), "--json"], env, 60_000);
  eq(second.code, 2, "a second free tunnel is refused (CLI error exit code)");
  assert(/1 tunnel at a time/.test(second.stderr), `limit message (${second.stderr})`);
  // Session expiry: GOAWAY session_expired → transparent reconnect to a fresh random URL.
  const { value: back } = await waitLine(t, jsonLine((v) => v.type === "status" && v.state === "connected"), { timeout: (FREE_SESSION_SECONDS + 30) * 1000, what: "reconnect after the session expired" });
  assert(back.publicUrl && back.publicUrl !== hello.publicUrl, `a new random URL after the session ended (${back.publicUrl})`);
  assert(t.lines.some((l) => l.includes('"state":"reconnecting"') && /session/i.test(l)), "the reconnect was caused by the session ending");
  const newHost = new URL(back.publicUrl).host;
  eq((await relayReq({ host: newHost, path: "/hello" })).status, 200, "the new URL works");
  const old = await relayReq({ host, path: "/hello" });
  assert(old.status >= 500, `the expired URL is offline (got ${old.status})`);
  await stopProc(t);
  return `${sub} → ${newHost.split(".")[0]} after ${FREE_SESSION_SECONDS} s; interstitial, 1-tunnel limit`;
}

async function scenarioHeartbeat() {
  if (!ctx.tunnel || ctx.tunnel.exited) throw new Error("the HTTP tunnel from scenario 1 isn't running");
  // Three missed heartbeats (25 s each) would make the client reconnect: stay idle past that.
  const idleFor = 80_000 - (Date.now() - ctx.tunnelSince);
  if (idleFor > 0) await sleep(idleFor);
  assert(!ctx.tunnel.lines.some((l) => l.includes('"type":"status"')), `no reconnect while idle (${ctx.tunnel.lines.filter((l) => l.includes('"type":"status"')).join(" | ")})`);
  const r = await relayReq({ host: `e2e-http.${DOMAIN}`, path: "/hello?after=idle" });
  eq(r.status, 200, "the tunnel still serves after > 75 s idle (heartbeats answered without waking the object)");
  return `${Math.round((Date.now() - ctx.tunnelSince) / 1000)} s connected, heartbeats answered, no reconnect`;
}

async function scenarioRestart() {
  if (!ctx.tunnel || ctx.tunnel.exited) throw new Error("the HTTP tunnel from scenario 1 isn't running");
  const from = ctx.tunnel.lines.length;
  const inbox = startProc("devall inbox listen (restart)", CLI, ["inbox", "listen", "--project", ctx.inboxProject, "--key", "e2e-inbox", "--json"], { env: cliEnv() });
  const { value: conn } = await waitLine(inbox, jsonLine((v) => v.type === "connected"), { what: "inbox connected" });
  const port = RELAY_PORT;
  await stopProc(ctx.relay);
  await waitLine(ctx.tunnel, jsonLine((v) => v.type === "status" && v.state === "reconnecting"), { from, what: "the tunnel noticing the relay went away" });
  await sleep(1500);
  await startRelay(ctx.tmp, port);
  await waitLine(ctx.tunnel, jsonLine((v) => v.type === "status" && v.state === "connected" && v.publicUrl === `https://e2e-http.${DOMAIN}`), { from, timeout: 60_000, what: "the tunnel reconnecting to the same URL" });
  const r = await relayReq({ host: `e2e-http.${DOMAIN}`, path: "/hello?after=restart" });
  eq(r.status, 200, "requests flow again after the relay restarted");
  // The inbox reconnects too and keeps its id; a webhook sent now arrives.
  const sent = await relayReq({ method: "POST", host: `hooks.${DOMAIN}`, path: `/${conn.inboxId}/after-restart`, body: Buffer.from("after restart") });
  eq(sent.status, 200, "webhook accepted after the restart");
  const id = sent.headers["x-devall-event-id"];
  await waitLine(inbox, jsonLine((v) => v.type === "event" && v.id === id), { timeout: 60_000, what: "the webhook delivered after the restart" });
  await stopProc(inbox);
  await stopProc(ctx.tunnel);
  return "tunnel + inbox reconnected to the restarted relay (same URL / inbox id)";
}

const SCENARIOS = [
  ["1 http tunnel", scenarioTunnel],
  ["2 tcp tunnel", scenarioTcp],
  ["3 hosted inbox", scenarioInbox],
  ["4 hosted mock", scenarioMock],
  ["5 shares", scenarioShare],
  ["6 monitors", scenarioMonitor],
  ["7 custom domain", scenarioCustomDomain],
  ["8 cloud load", scenarioCloudLoad],
  ["9 free tier", scenarioFreeTier],
  ["10 idle heartbeat", scenarioHeartbeat],
  ["11 relay restart", scenarioRestart],
];

// ---------- main ----------

async function main() {
  if (!fs.existsSync(path.join(WORKER, "node_modules", "wrangler", "bin", "wrangler.js"))) throw new Error("run `npm install` in relay/worker first");
  if (!fs.existsSync(CLI)) {
    console.log("building the CLI (cargo build -p devall-cli)…");
    execFileSync("cargo", ["build", "-p", "devall-cli"], { cwd: ROOT, stdio: "inherit" });
  }
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "devall-relay-e2e-"));
  ctx.tmp = tmp;
  ctx.cleanup = [];
  for (const name of ["inbox", "mock", "load"]) {
    const dir = path.join(tmp, `${name}-project`);
    execFileSync(CLI, ["init", dir, "--name", `e2e ${name}`], { stdio: "ignore" });
    ctx[`${name}Project`] = dir;
  }
  const results = [];
  try {
    console.log(`starting the relay (wrangler dev --local) — temp folder ${tmp}`);
    const started = Date.now();
    await startRelay(tmp);
    console.log(`relay up on http://127.0.0.1:${RELAY_PORT} after ${((Date.now() - started) / 1000).toFixed(1)} s (PUBLIC_DOMAIN ${DOMAIN}, routed by Host header)`);
    ctx.app = await startApp();
    ctx.cleanup.push(ctx.app.close);
    for (const [name, fn] of SCENARIOS) {
      if (FILTER.length && !FILTER.some((f) => name.includes(f))) continue;
      const t0 = Date.now();
      try {
        const detail = await Promise.race([fn(), sleep(240_000).then(() => Promise.reject(new Error("scenario timed out after 240 s")))]);
        results.push([name, true]);
        console.log(`PASS  ${name}  (${((Date.now() - t0) / 1000).toFixed(1)} s) — ${detail}`);
      } catch (e) {
        results.push([name, false]);
        console.log(`FAIL  ${name}  (${((Date.now() - t0) / 1000).toFixed(1)} s) — ${e.stack ?? e}`);
      }
    }
  } finally {
    for (const rec of [...procs].reverse()) await stopProc(rec);
    for (const c of ctx.cleanup.reverse()) await c().catch(() => {});
    await closeLogs();
    const failed = results.filter((r) => !r[1]).length;
    if (failed && fs.existsSync(path.join(tmp, "wrangler.log"))) {
      const log = fs.readFileSync(path.join(tmp, "wrangler.log"), "utf8").split(/\r?\n/);
      console.log(`--- wrangler.log (last 40 lines)\n${log.slice(-40).join("\n")}`);
    }
    if (!process.env.KEEP_TMP) {
      // workerd keeps its SQLite files locked for a few seconds after being killed (Windows).
      const until = Date.now() + 30_000;
      for (;;) {
        try {
          fs.rmSync(tmp, { recursive: true, force: true });
          break;
        } catch (e) {
          if (Date.now() > until) {
            console.log(`(couldn't remove ${tmp}: ${e.code ?? e})`);
            break;
          }
          await sleep(1000);
        }
      }
    } else console.log(`kept ${tmp}`);
    console.log(`\n${results.length - failed}/${results.length} scenarios passed`);
    process.exitCode = failed || !results.length ? 1 : 0;
  }
}

for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => {
    for (const rec of procs) killTree(rec);
    process.exit(130);
  });
}
process.on("exit", () => {
  for (const rec of procs) killTree(rec);
});

main().catch((e) => {
  console.error(e.stack ?? e);
  for (const rec of procs) killTree(rec);
  process.exitCode = 1;
});
