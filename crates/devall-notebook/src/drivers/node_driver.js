// /dev/all notebook kernel for Node.js (started by the app; not meant to be run by hand).
//
// Protocol: the host writes JSON lines on stdin ({"t":"exec","id","code","cwd","env"} and
// {"t":"interrupt"}); the kernel answers with lines on stdout that start with "\x1eDEVALL" followed
// by JSON ({"t":"ready"|"stream"|"display"|"error"|"done", "id", ...}). Anything else on stdout or
// stderr (child processes, process.stdout.write) is the running cell's raw output.
//
// Cells are evaluated in V8's REPL mode through the in-process inspector (what the DevTools
// console uses): top-level `await`, and `let`/`const` can be re-declared when a cell is re-run.
// Variables live in the global scope of this process, one process per notebook. A worker thread
// reads stdin so that a busy cell can be interrupted (Runtime.terminateExecution) while the
// variables survive.
'use strict';

const { isMainThread } = require('node:worker_threads');

const MARK = '\x1eDEVALL';

if (isMainThread) main();
else stdinWorker();

function stdinWorker() {
  const { parentPort, workerData } = require('node:worker_threads');
  const inspector = require('node:inspector');
  const fs = require('node:fs');
  const beat = new Float64Array(workerData.heartbeat);
  let session = null;
  try {
    session = new inspector.Session();
    session.connectToMainThread();
  } catch {
    session = null;
  }
  const handle = (line) => {
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    if (msg.t === 'interrupt') {
      // A cell is running and the main thread hasn't ticked lately: it's stuck in synchronous
      // code, which only the inspector can stop.
      const blocked = beat[1] === 1 && Date.now() - beat[0] > 300;
      if (blocked && session) session.post('Runtime.terminateExecution', () => {});
    }
    parentPort.postMessage(msg);
  };
  let buf = '';
  const input = fs.createReadStream(null, { fd: 0, encoding: 'utf8', highWaterMark: 1 << 16 });
  input.on('data', (chunk) => {
    buf += chunk;
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      if (line.trim()) handle(line);
    }
  });
  const gone = () => {
    parentPort.postMessage({ t: 'eof' });
    // The app went away; if the main thread is stuck, end the process from here.
    setTimeout(() => process.kill(process.pid), 1000).unref();
  };
  input.on('end', gone);
  input.on('error', gone);
}

function main() {
  const fs = require('node:fs');
  const path = require('node:path');
  const util = require('node:util');
  const url = require('node:url');
  const inspector = require('node:inspector');
  const Module = require('node:module');
  const { Worker } = require('node:worker_threads');
  const { Console } = require('node:console');
  const { Writable } = require('node:stream');

  const DRIVER = __filename;
  let current = 0;
  const beatBuf = new SharedArrayBuffer(16);
  const beat = new Float64Array(beatBuf);
  beat[0] = Date.now();
  setInterval(() => (beat[0] = Date.now()), 100);

  const sleeper = new Int32Array(new SharedArrayBuffer(4));
  function writeAll(text) {
    const b = Buffer.from(text, 'utf8');
    let off = 0;
    while (off < b.length) {
      try {
        off += fs.writeSync(1, b, off, b.length - off);
      } catch (e) {
        if (e.code !== 'EAGAIN') throw e;
        Atomics.wait(sleeper, 0, 0, 2);
      }
    }
  }
  function send(msg) {
    if (msg.id === undefined) msg.id = current;
    let text;
    try {
      text = JSON.stringify(msg);
    } catch {
      text = JSON.stringify({ ...msg, data: { 'text/plain': inspect(msg.data && msg.data['text/plain']) } });
    }
    writeAll(MARK + text + '\n');
  }

  const inspect = (v) => util.inspect(v, { depth: 4, colors: false, maxArrayLength: 200, maxStringLength: 20000, breakLength: 100 });
  const stream = (name) =>
    new Writable({
      decodeStrings: false,
      write(chunk, _enc, cb) {
        send({ t: 'stream', name, text: typeof chunk === 'string' ? chunk : Buffer.from(chunk).toString('utf8') });
        cb();
      },
    });
  Object.defineProperty(globalThis, 'console', {
    value: new Console({ stdout: stream('stdout'), stderr: stream('stderr'), colorMode: false }),
    writable: true,
    configurable: true,
    enumerable: false,
  });

  // ---------- display ----------
  const RICH = Symbol.for('devall.display');
  const bundle = (v) => (v && typeof v === 'object' && v[RICH] ? v[RICH] : { 'text/plain': inspect(v) });
  function display(...values) {
    for (const v of values) send({ t: 'display', data: bundle(v), result: false });
  }
  const rich = (data) => ({ [RICH]: data, toString: () => data['text/plain'] || '' });
  display.html = (html) => display(rich({ 'text/html': String(html), 'text/plain': '[HTML]' }));
  display.markdown = (md) => display(rich({ 'text/markdown': String(md), 'text/plain': String(md) }));
  display.json = (value) => display(rich({ 'application/json': JSON.parse(JSON.stringify(value ?? null)), 'text/plain': inspect(value) }));
  display.image = (base64, mime = 'image/png') => display(rich({ [mime]: String(base64), 'text/plain': `[${mime}]` }));
  globalThis.display = display;

  // ---------- modules ----------
  const isBuiltin = (s) => s.startsWith('node:') || (Module.isBuiltin ? Module.isBuiltin(s) : Module.builtinModules.includes(s));
  globalThis.__devallImport = (spec) => {
    spec = String(spec);
    if (isBuiltin(spec) || /^(file|data|https?):/.test(spec)) return import(spec);
    if (spec.startsWith('.') || path.isAbsolute(spec)) return import(url.pathToFileURL(path.resolve(process.cwd(), spec)).href);
    const req = Module.createRequire(path.join(process.cwd(), '__devall_cell__.js'));
    return import(url.pathToFileURL(req.resolve(spec)).href);
  };
  globalThis.__devallDefault = (m) => (m && typeof m === 'object' && 'default' in m ? m.default : m);
  let modCounter = 0;
  /** Static `import … from '…'` lines → awaited dynamic imports (scripts can't use `import`). */
  function rewriteImports(code) {
    code = code.replace(
      /^([ \t]*)import\s+(?:([\w$]+)\s*,?\s*)?(?:\*\s*as\s+([\w$]+)|\{([^}]*)\})?\s*from\s*(['"])([^'"\n]+)\5[ \t]*;?/gm,
      (_all, ind, def, ns, named, _q, mod) => {
        const tmp = `__devallMod${++modCounter}`;
        const parts = [`${ind}const ${tmp} = await __devallImport(${JSON.stringify(mod)});`];
        if (def) parts.push(`const ${def} = __devallDefault(${tmp});`);
        if (ns) parts.push(`const ${ns} = ${tmp};`);
        if (named !== undefined) {
          const names = named
            .split(',')
            .map((s) => s.trim())
            .filter(Boolean)
            .map((s) => s.replace(/^([\w$]+)\s+as\s+([\w$]+)$/, '$1: $2'));
          parts.push(`const { ${names.join(', ')} } = ${tmp};`);
        }
        return parts.join(' ');
      },
    );
    code = code.replace(/^([ \t]*)import\s*(['"])([^'"\n]+)\2[ \t]*;?/gm, (_all, ind, _q, mod) => `${ind}await __devallImport(${JSON.stringify(mod)});`);
    return code.replace(/(^|[^.\w$])import\s*\(/g, '$1__devallImport(');
  }

  // ---------- evaluation ----------
  const session = new inspector.Session();
  session.connect();
  const post = (method, params) => new Promise((resolve) => session.post(method, params, (err, r) => resolve({ err, r })));

  async function valueOf(remote) {
    if (!remote) return undefined;
    if (remote.objectId) {
      await post('Runtime.callFunctionOn', { objectId: remote.objectId, functionDeclaration: 'function () { globalThis.__devallLast = this; }' });
      const v = globalThis.__devallLast;
      delete globalThis.__devallLast;
      return v;
    }
    if ('value' in remote) return remote.value;
    if (remote.unserializableValue !== undefined) {
      const u = remote.unserializableValue;
      return remote.type === 'bigint' ? BigInt(u.slice(0, -1)) : Number(u);
    }
    return remote.type === 'undefined' ? undefined : remote.description;
  }

  const noise = (line) => line.includes('node:inspector') || line.includes(DRIVER) || /^\s+at (Session\.post|post|run)\b/.test(line);
  async function sendException(details) {
    const v = await valueOf(details.exception);
    if (v instanceof Error) {
      const stack = String(v.stack || `${v.name}: ${v.message}`).split('\n').filter((l) => !noise(l));
      send({ t: 'error', ename: v.name || 'Error', evalue: v.message || '', traceback: stack });
    } else {
      const text = details.exception ? inspect(v) : details.text || 'Error';
      send({ t: 'error', ename: 'Uncaught', evalue: text, traceback: [`Uncaught ${text}`] });
    }
  }

  let running = null;
  async function run(msg) {
    current = msg.id;
    const saved = {};
    for (const [k, v] of Object.entries(msg.env || {})) {
      saved[k] = process.env[k];
      process.env[k] = v;
    }
    if (msg.cwd) {
      try {
        process.chdir(msg.cwd);
      } catch (e) {
        send({ t: 'stream', name: 'stderr', text: `warning: can't change to ${msg.cwd}: ${e.message}\n` });
      }
    }
    globalThis.require = Module.createRequire(path.join(process.cwd(), '__devall_cell__.js'));
    let ok = true;
    let interrupted = false;
    const soft = new Promise((resolve) => (running = { id: msg.id, interrupt: () => resolve({ soft: true }) }));
    beat[1] = 1;
    beat[0] = Date.now();
    const evaluated = post('Runtime.evaluate', { expression: rewriteImports(msg.code || ''), replMode: true, awaitPromise: true, objectGroup: 'cell', silent: false });
    const out = await Promise.race([evaluated, soft]);
    beat[1] = 0;
    running = null;
    try {
      if (out.soft || out.err) {
        ok = false;
        interrupted = true;
        const note = out.soft ? ' (pending async work may still run — restart the kernel to stop it)' : '';
        send({ t: 'error', ename: 'Interrupted', evalue: `the cell was interrupted${note}`, traceback: [`Interrupted${note}`] });
      } else if (out.r.exceptionDetails) {
        ok = false;
        await sendException(out.r.exceptionDetails);
      } else if (out.r.result && out.r.result.type !== 'undefined') {
        const v = await valueOf(out.r.result);
        if (v !== undefined) {
          globalThis._ = v;
          send({ t: 'display', data: bundle(v), result: true });
        }
      }
    } catch (e) {
      ok = false;
      send({ t: 'error', ename: 'InternalError', evalue: String(e && e.message), traceback: [String(e && e.stack)] });
    } finally {
      for (const [k, v] of Object.entries(saved)) {
        if (v === undefined) delete process.env[k];
        else process.env[k] = v;
      }
      session.post('Runtime.releaseObjectGroup', { objectGroup: 'cell' }, () => {});
    }
    send({ t: 'done', ok, interrupted });
  }

  const queue = [];
  let busy = false;
  async function pump() {
    if (busy) return;
    busy = true;
    while (queue.length) await run(queue.shift());
    busy = false;
  }

  process.on('uncaughtException', (e) => send({ t: 'stream', name: 'stderr', text: `Uncaught ${e && e.stack ? e.stack : inspect(e)}\n` }));
  process.on('unhandledRejection', (e) => send({ t: 'stream', name: 'stderr', text: `Unhandled promise rejection: ${e && e.stack ? e.stack : inspect(e)}\n` }));

  const worker = new Worker(__filename, { workerData: { heartbeat: beatBuf } });
  worker.unref();
  worker.on('message', (msg) => {
    if (msg.t === 'exec') {
      queue.push(msg);
      pump();
    } else if (msg.t === 'interrupt') {
      if (running) running.interrupt();
    } else if (msg.t === 'eof') {
      process.exit(0);
    }
  });
  worker.on('error', (e) => send({ t: 'stream', name: 'stderr', text: `kernel input error: ${e.message}\n` }));
  worker.on('exit', () => process.exit(0));

  send({ t: 'ready', id: 0, version: process.versions.node });
}
