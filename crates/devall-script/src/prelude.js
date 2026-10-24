// /dev/all script prelude. Evaluated in every fresh script context before the user's code: builds
// `pm`, a chai-style `expect`, CryptoJS, console, timers and the Postman/Bruno compatibility shims
// on top of a few Rust natives (`__devall_native`, removed from the global scope at the end).
// Results are read back by Rust through `__devall.collect()`.
(function (N, I, BODY) {
  'use strict';
  const G = globalThis;
  const stringify = JSON.stringify;
  const parseJson = JSON.parse;
  const hasOwn = (o, k) => o != null && Object.prototype.hasOwnProperty.call(o, k);
  const INSPECT = Symbol('devall.inspect');
  const EVENT = I.event; // 'prerequest' | 'test'
  const PRE = EVENT === 'prerequest';
  const define = (name, value) => Object.defineProperty(G, name, { value, writable: true, configurable: true, enumerable: false });
  const warn = (msg) => N.log('warn', msg);
  const unsupported = (what, hint) => {
    throw new Error(`${what} is not supported in /dev/all scripts${hint ? ` — ${hint}` : ''}`);
  };

  // ---------------------------------------------------------------- formatting

  function typeOf(v) {
    if (v === null) return 'null';
    if (Array.isArray(v)) return 'array';
    const t = typeof v;
    if (t !== 'object') return t;
    return Object.prototype.toString.call(v).slice(8, -1).toLowerCase();
  }

  const IDENT = /^[A-Za-z_$][\w$]*$/;
  function quote(s) {
    return "'" + s.replace(/\\/g, '\\\\').replace(/'/g, "\\'").replace(/\n/g, '\\n').replace(/\r/g, '\\r').replace(/\t/g, '\\t') + "'";
  }

  /** Node/chai-style rendering: `{ a: 1, b: 'x' }`, `[ 1, 2 ]`. */
  function inspect(value, depth = 2, maxItems = 100) {
    const seen = [];
    const fmt = (x, d) => {
      switch (typeof x) {
        case 'string':
          return quote(x);
        case 'number':
          return Object.is(x, -0) ? '-0' : String(x);
        case 'bigint':
          return `${x}n`;
        case 'boolean':
        case 'undefined':
          return String(x);
        case 'symbol':
          return x.toString();
        case 'function':
          return x.name ? `[Function: ${x.name}]` : '[Function]';
      }
      if (x === null) return 'null';
      if (seen.includes(x)) return '[Circular]';
      if (typeof x[INSPECT] === 'function') {
        seen.push(x);
        try {
          return fmt(x[INSPECT](), d);
        } finally {
          seen.pop();
        }
      }
      if (x instanceof Date) return isNaN(x.getTime()) ? 'Invalid Date' : x.toISOString();
      if (x instanceof RegExp) return String(x);
      if (x instanceof Error) return `[${x.name || 'Error'}: ${x.message}]`;
      seen.push(x);
      try {
        if (Array.isArray(x) || ArrayBuffer.isView(x)) {
          const prefix = Array.isArray(x) ? '' : `${x.constructor.name}(${x.length}) `;
          if (!x.length) return `${prefix}[]`;
          if (d > depth) return Array.isArray(x) ? '[Array]' : `[${x.constructor.name}]`;
          const items = Array.prototype.slice.call(x, 0, maxItems).map((i) => fmt(i, d + 1));
          if (x.length > maxItems) items.push(`... ${x.length - maxItems} more items`);
          return `${prefix}[ ${items.join(', ')} ]`;
        }
        if (x instanceof Map) {
          if (!x.size) return 'Map(0) {}';
          if (d > depth) return '[Map]';
          return `Map(${x.size}) { ${[...x].slice(0, maxItems).map(([k, v]) => `${fmt(k, d + 1)} => ${fmt(v, d + 1)}`).join(', ')} }`;
        }
        if (x instanceof Set) {
          if (!x.size) return 'Set(0) {}';
          if (d > depth) return '[Set]';
          return `Set(${x.size}) { ${[...x].slice(0, maxItems).map((v) => fmt(v, d + 1)).join(', ')} }`;
        }
        const keys = Object.keys(x);
        const ctor = Object.getPrototypeOf(x);
        const cname = ctor && ctor !== Object.prototype && ctor.constructor && ctor.constructor.name ? ctor.constructor.name : '';
        if (!keys.length) return cname ? `${cname} {}` : '{}';
        if (d > depth) return `[${cname || 'Object'}]`;
        const parts = keys.slice(0, maxItems).map((k) => `${IDENT.test(k) ? k : quote(k)}: ${fmt(x[k], d + 1)}`);
        if (keys.length > maxItems) parts.push(`... ${keys.length - maxItems} more items`);
        return `${cname ? cname + ' ' : ''}{ ${parts.join(', ')} }`;
      } finally {
        seen.pop();
      }
    };
    return fmt(value, 0);
  }

  /** chai's objDisplay: long values are summarised. */
  function objDisplay(obj) {
    const str = inspect(obj);
    if (str.length < 40) return str;
    const t = typeOf(obj);
    if (t === 'function') return obj.name ? `[Function: ${obj.name}]` : '[Function]';
    if (t === 'array') return `[ Array(${obj.length}) ]`;
    if (t === 'object') {
      const keys = Object.keys(obj);
      return `{ Object (${keys.length > 2 ? keys.slice(0, 2).join(', ') + ', ...' : keys.join(', ')}) }`;
    }
    return str;
  }

  function errorText(e) {
    if (e instanceof Error) return `${e.name || 'Error'}: ${e.message}`;
    return typeof e === 'string' ? e : inspect(e);
  }

  // ---------------------------------------------------------------- console

  function formatArgs(args) {
    const out = [];
    let i = 0;
    if (typeof args[0] === 'string' && args.length > 1 && args[0].includes('%')) {
      i = 1;
      out.push(
        args[0].replace(/%[sdifjoOc%]/g, (m) => {
          if (m === '%%') return '%';
          if (i >= args.length) return m;
          const a = args[i++];
          switch (m) {
            case '%s':
              return typeof a === 'string' ? a : typeof a === 'object' && a !== null ? inspect(a, 1) : String(a);
            case '%d':
              return typeof a === 'bigint' ? `${a}n` : String(Number(a));
            case '%i':
              return typeof a === 'bigint' ? `${a}n` : String(Math.trunc(Number(a)));
            case '%f':
              return String(parseFloat(a));
            case '%j':
              try {
                return stringify(a);
              } catch {
                return '[Circular]';
              }
            case '%c':
              return '';
            default:
              return inspect(a, 4);
          }
        }),
      );
    }
    for (; i < args.length; i++) {
      const a = args[i];
      out.push(typeof a === 'string' ? a : a instanceof Error ? errorText(a) : inspect(a, 4));
    }
    return out.join(' ');
  }

  let indent = '';
  const counters = Object.create(null);
  const started = Object.create(null);
  const emit = (level, args) => N.log(level, indent + formatArgs(args));
  define('console', {
    log: (...a) => emit('log', a),
    info: (...a) => emit('info', a),
    warn: (...a) => emit('warn', a),
    error: (...a) => emit('error', a),
    debug: (...a) => emit('debug', a),
    trace: (...a) => emit('debug', a),
    dir: (o) => N.log('log', indent + inspect(o, 4)),
    table: (o) => N.log('log', indent + inspect(o, 4)),
    assert: (cond, ...a) => {
      if (!cond) N.log('error', indent + 'Assertion failed' + (a.length ? ': ' + formatArgs(a) : ''));
    },
    count: (label = 'default') => {
      counters[label] = (counters[label] || 0) + 1;
      N.log('log', `${indent}${label}: ${counters[label]}`);
    },
    countReset: (label = 'default') => {
      delete counters[label];
    },
    time: (label = 'default') => {
      started[label] = Date.now();
    },
    timeLog: (label = 'default', ...a) => {
      if (label in started) N.log('log', `${indent}${label}: ${Date.now() - started[label]}ms${a.length ? ' ' + formatArgs(a) : ''}`);
    },
    timeEnd: (label = 'default') => {
      if (label in started) {
        N.log('log', `${indent}${label}: ${Date.now() - started[label]}ms`);
        delete started[label];
      }
    },
    group: (...a) => {
      if (a.length) emit('log', a);
      indent += '  ';
    },
    groupCollapsed: (...a) => {
      if (a.length) emit('log', a);
      indent += '  ';
    },
    groupEnd: () => {
      indent = indent.slice(2);
    },
    clear: () => {},
  });

  // ---------------------------------------------------------------- helpers

  function deepEqual(a, b, memo) {
    if (a === b) return a !== 0 || 1 / a === 1 / b;
    if (a !== a && b !== b) return true; // NaN
    if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
    const ta = typeOf(a);
    if (ta !== typeOf(b)) return false;
    memo = memo || [];
    for (const [x, y] of memo) if (x === a && y === b) return true;
    memo.push([a, b]);
    switch (ta) {
      case 'date':
        return a.getTime() === b.getTime() || (isNaN(a.getTime()) && isNaN(b.getTime()));
      case 'regexp':
        return String(a) === String(b);
      case 'error':
        return false;
      case 'boolean':
      case 'number':
      case 'string':
        return Object.is(a.valueOf(), b.valueOf());
      case 'map':
        if (a.size !== b.size) return false;
        for (const [k, v] of a) if (!b.has(k) || !deepEqual(v, b.get(k), memo)) return false;
        return true;
      case 'set':
        if (a.size !== b.size) return false;
        for (const v of a) if (!b.has(v) && ![...b].some((w) => deepEqual(v, w, memo))) return false;
        return true;
    }
    if (Array.isArray(a) || ArrayBuffer.isView(a)) {
      if (a.length !== b.length) return false;
      for (let i = 0; i < a.length; i++) if (!deepEqual(a[i], b[i], memo)) return false;
      return true;
    }
    const ka = Object.keys(a);
    const kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    for (const k of ka) if (!hasOwn(b, k) || !deepEqual(a[k], b[k], memo)) return false;
    return true;
  }

  /** `a.b[0].c`, `a["x y"]`, `$.a.b` (leading `$` ignored), or an array of keys. */
  function parsePath(path) {
    if (Array.isArray(path)) return path.map(String);
    const parts = [];
    String(path)
      .replace(/^\$(?=[.[]|$)/, '')
      .replace(/\[\s*(?:(\d+)|(["'])(.*?)\2)\s*\]|[^.[\]]+/g, (m, idx, _q, str) => {
        parts.push(idx !== undefined ? idx : str !== undefined ? str : m);
        return m;
      });
    return parts;
  }

  function pathInfo(obj, path) {
    const parts = parsePath(path);
    let cur = obj;
    for (const p of parts) {
      if (cur === null || cur === undefined || !(p in Object(cur))) return { exists: false, value: undefined };
      cur = cur[p];
    }
    return { exists: true, value: cur };
  }

  /** Variable values are strings on disk; non-strings are serialised when they leave the script. */
  function asString(v) {
    if (typeof v === 'string') return v;
    if (v === undefined) return '';
    if (typeof v === 'object') {
      try {
        return stringify(v);
      } catch {
        return String(v);
      }
    }
    return String(v);
  }

  const decode = (s) => {
    try {
      return decodeURIComponent(s.replace(/\+/g, ' '));
    } catch {
      return s;
    }
  };
  const hexToBytes = (h) => {
    const out = new Uint8Array(h.length >> 1);
    for (let i = 0; i < out.length; i++) out[i] = parseInt(h.substr(i * 2, 2), 16);
    return out;
  };
  const bytesToHex = (b) => {
    let s = '';
    for (let i = 0; i < b.length; i++) s += (b[i] & 0xff).toString(16).padStart(2, '0');
    return s;
  };
  const latin1ToHex = (s) => {
    let h = '';
    for (let i = 0; i < s.length; i++) h += (s.charCodeAt(i) & 0xff).toString(16).padStart(2, '0');
    return h;
  };
  const hexToLatin1 = (h) => {
    let s = '';
    for (let i = 0; i < h.length; i += 2) s += String.fromCharCode(parseInt(h.substr(i, 2), 16));
    return s;
  };

  // ---------------------------------------------------------------- JSON schema (subset)

  /** Draft-07 subset; returns the first error as `{ path, message }` or null. */
  function validateSchema(data, schema, root = schema, path = 'data') {
    if (schema === true || schema === undefined || schema === null) return null;
    if (schema === false) return { path, message: 'is not allowed' };
    if (schema.$ref) {
      const m = /^#\/(?:definitions|\$defs)\/(.+)$/.exec(schema.$ref);
      const target = m && ((root.definitions && root.definitions[m[1]]) || (root.$defs && root.$defs[m[1]]));
      if (!target) return { path, message: `unsupported $ref ${schema.$ref}` };
      return validateSchema(data, target, root, path);
    }
    const t = typeOf(data);
    if (schema.type !== undefined) {
      const types = Array.isArray(schema.type) ? schema.type : [schema.type];
      const ok = types.some((ty) => ty === t || (ty === 'integer' && t === 'number' && Number.isInteger(data)) || (ty === 'number' && t === 'number'));
      if (!ok) return { path, message: `should be ${types.join(' or ')} but is ${t}` };
    }
    if (schema.enum && !schema.enum.some((e) => deepEqual(e, data))) return { path, message: `should be one of ${inspect(schema.enum)}` };
    if ('const' in schema && !deepEqual(schema.const, data)) return { path, message: `should be ${inspect(schema.const)}` };
    if (t === 'number') {
      if (schema.minimum !== undefined && data < schema.minimum) return { path, message: `should be >= ${schema.minimum}` };
      if (schema.maximum !== undefined && data > schema.maximum) return { path, message: `should be <= ${schema.maximum}` };
      if (typeof schema.exclusiveMinimum === 'number' && data <= schema.exclusiveMinimum) return { path, message: `should be > ${schema.exclusiveMinimum}` };
      if (typeof schema.exclusiveMaximum === 'number' && data >= schema.exclusiveMaximum) return { path, message: `should be < ${schema.exclusiveMaximum}` };
      if (schema.multipleOf && Math.abs(data / schema.multipleOf - Math.round(data / schema.multipleOf)) > 1e-9) return { path, message: `should be a multiple of ${schema.multipleOf}` };
    }
    if (t === 'string') {
      if (schema.minLength !== undefined && data.length < schema.minLength) return { path, message: `should have at least ${schema.minLength} characters` };
      if (schema.maxLength !== undefined && data.length > schema.maxLength) return { path, message: `should have at most ${schema.maxLength} characters` };
      if (schema.pattern && !new RegExp(schema.pattern, 'u').test(data)) return { path, message: `should match pattern ${schema.pattern}` };
    }
    if (t === 'array') {
      if (schema.minItems !== undefined && data.length < schema.minItems) return { path, message: `should have at least ${schema.minItems} items` };
      if (schema.maxItems !== undefined && data.length > schema.maxItems) return { path, message: `should have at most ${schema.maxItems} items` };
      if (schema.uniqueItems) for (let i = 0; i < data.length; i++) for (let j = i + 1; j < data.length; j++) if (deepEqual(data[i], data[j])) return { path, message: `items ${i} and ${j} are identical` };
      if (Array.isArray(schema.items)) {
        for (let i = 0; i < data.length; i++) {
          const sub = i < schema.items.length ? schema.items[i] : schema.additionalItems;
          const e = validateSchema(data[i], sub, root, `${path}[${i}]`);
          if (e) return e;
        }
      } else if (schema.items !== undefined) {
        for (let i = 0; i < data.length; i++) {
          const e = validateSchema(data[i], schema.items, root, `${path}[${i}]`);
          if (e) return e;
        }
      }
      if (schema.contains !== undefined && !data.some((d) => !validateSchema(d, schema.contains, root))) return { path, message: 'should contain a matching item' };
    }
    if (t === 'object') {
      const keys = Object.keys(data);
      for (const r of schema.required || []) if (!hasOwn(data, r)) return { path, message: `should have required property '${r}'` };
      if (schema.minProperties !== undefined && keys.length < schema.minProperties) return { path, message: `should have at least ${schema.minProperties} properties` };
      if (schema.maxProperties !== undefined && keys.length > schema.maxProperties) return { path, message: `should have at most ${schema.maxProperties} properties` };
      const props = schema.properties || {};
      const patterns = Object.keys(schema.patternProperties || {}).map((p) => [new RegExp(p, 'u'), schema.patternProperties[p]]);
      for (const k of keys) {
        const sub = IDENT.test(k) ? `${path}.${k}` : `${path}[${quote(k)}]`;
        let known = false;
        if (hasOwn(props, k)) {
          known = true;
          const e = validateSchema(data[k], props[k], root, sub);
          if (e) return e;
        }
        for (const [re, s] of patterns) {
          if (re.test(k)) {
            known = true;
            const e = validateSchema(data[k], s, root, sub);
            if (e) return e;
          }
        }
        if (!known && schema.additionalProperties !== undefined) {
          if (schema.additionalProperties === false) return { path, message: `should not have additional property '${k}'` };
          const e = validateSchema(data[k], schema.additionalProperties, root, sub);
          if (e) return e;
        }
      }
    }
    if (schema.allOf) for (const s of schema.allOf) {
      const e = validateSchema(data, s, root, path);
      if (e) return e;
    }
    if (schema.anyOf && !schema.anyOf.some((s) => !validateSchema(data, s, root, path))) return { path, message: 'should match some schema in anyOf' };
    if (schema.oneOf && schema.oneOf.filter((s) => !validateSchema(data, s, root, path)).length !== 1) return { path, message: 'should match exactly one schema in oneOf' };
    if (schema.not !== undefined && !validateSchema(data, schema.not, root, path)) return { path, message: 'should not match the schema in not' };
    if (schema.if !== undefined) {
      const branch = validateSchema(data, schema.if, root, path) ? schema.else : schema.then;
      if (branch !== undefined) {
        const e = validateSchema(data, branch, root, path);
        if (e) return e;
      }
    }
    return null;
  }

  // ---------------------------------------------------------------- chai-style assertions

  class AssertionError extends Error {
    constructor(message, props) {
      super(message);
      this.name = 'AssertionError';
      if (props) Object.assign(this, props);
    }
  }

  function Assertion(obj, msg) {
    if (!(this instanceof Assertion)) return new Assertion(obj, msg);
    Object.defineProperty(this, '__flags', { value: { object: obj, message: msg }, writable: true });
  }
  function flag(a, key, value) {
    if (arguments.length === 3) a.__flags[key] = value;
    return a.__flags[key];
  }
  Assertion.prototype.assert = function (expr, msg, negMsg, expected, actual) {
    const negate = flag(this, 'negate');
    if (negate ? !expr : expr) return;
    const act = arguments.length > 4 ? actual : flag(this, 'object');
    let m = negate ? negMsg : msg;
    if (typeof m === 'function') m = m();
    m = String(m)
      .replace(/#\{this\}/g, () => objDisplay(flag(this, 'object')))
      .replace(/#\{act\}/g, () => objDisplay(act))
      .replace(/#\{exp\}/g, () => objDisplay(expected));
    const custom = flag(this, 'message');
    throw new AssertionError(custom ? `${custom}: ${m}` : m, { actual: act, expected });
  };
  Assertion.prototype[INSPECT] = function () {
    return { Assertion: flag(this, 'object') };
  };

  function addProperty(name, getter) {
    Object.defineProperty(Assertion.prototype, name, {
      configurable: true,
      get() {
        const r = getter.call(this);
        return r === undefined ? this : r;
      },
    });
  }
  function addMethod(names, fn) {
    for (const name of [].concat(names)) {
      Assertion.prototype[name] = function (...args) {
        const r = fn.apply(this, args);
        return r === undefined ? this : r;
      };
    }
  }
  function addChainableMethod(names, method, chaining) {
    for (const name of [].concat(names)) {
      Object.defineProperty(Assertion.prototype, name, {
        configurable: true,
        get() {
          if (chaining) chaining.call(this);
          const self = this;
          const fn = function (...args) {
            const r = method.apply(self, args);
            return r === undefined ? self : r;
          };
          Object.setPrototypeOf(fn, self);
          return fn;
        },
      });
    }
  }

  for (const w of ['to', 'be', 'been', 'is', 'that', 'which', 'and', 'has', 'have', 'with', 'at', 'of', 'same', 'but', 'does', 'still', 'also']) addProperty(w, function () {});
  addProperty('not', function () {
    flag(this, 'negate', !flag(this, 'negate'));
  });
  for (const f of ['deep', 'own', 'nested', 'ordered']) addProperty(f, function () {
    flag(this, f, true);
  });
  addProperty('any', function () {
    flag(this, 'any', true);
    flag(this, 'all', false);
  });
  addProperty('all', function () {
    flag(this, 'all', true);
    flag(this, 'any', false);
  });

  const isResponse = (o) => o instanceof Response;
  const sizeOf = (o) => (o instanceof Map || o instanceof Set ? o.size : o == null ? undefined : o.length);
  const article = (t) => (/^[aeiou]/.test(t) ? 'an ' : 'a ');

  addChainableMethod(['a', 'an'], function (type, msg) {
    if (msg) flag(this, 'message', msg);
    type = String(type).toLowerCase();
    this.assert(typeOf(flag(this, 'object')) === type, `expected #{this} to be ${article(type)}${type}`, `expected #{this} not to be ${article(type)}${type}`);
  });

  addChainableMethod(
    ['include', 'includes', 'contain', 'contains'],
    function (val, msg) {
      if (msg) flag(this, 'message', msg);
      const obj = flag(this, 'object');
      const deep = flag(this, 'deep');
      const desc = deep ? 'deep ' : '';
      let ok;
      switch (typeOf(obj)) {
        case 'string':
          ok = obj.indexOf(val) !== -1;
          break;
        case 'array':
          ok = deep ? obj.some((i) => deepEqual(i, val)) : obj.includes(val);
          break;
        case 'map':
          ok = [...obj.values()].some((v) => (deep ? deepEqual(v, val) : v === val));
          break;
        case 'set':
          ok = deep ? [...obj].some((v) => deepEqual(v, val)) : obj.has(val);
          break;
        default:
          if (obj === null || typeof obj !== 'object') throw new AssertionError(`object tested must be an array, a map, an object, a set or a string, but ${typeOf(obj)} given`);
          if (val === null || typeof val !== 'object') throw new AssertionError(`the given combination of arguments (${typeOf(obj)} and ${typeOf(val)}) is invalid for this assertion`);
          ok = Object.keys(val).every((k) => hasOwn(obj, k) && (deep ? deepEqual(obj[k], val[k]) : obj[k] === val[k]));
      }
      this.assert(ok, `expected #{this} to ${desc}include #{exp}`, `expected #{this} to not ${desc}include #{exp}`, val);
    },
    function () {
      flag(this, 'contains', true);
    },
  );

  addChainableMethod(
    ['length', 'lengthOf'],
    function (n, msg) {
      if (msg) flag(this, 'message', msg);
      const obj = flag(this, 'object');
      const isMapSet = obj instanceof Map || obj instanceof Set;
      const len = sizeOf(obj);
      if (len === undefined) throw new AssertionError(`expected ${objDisplay(obj)} to have property 'length'`);
      const what = isMapSet ? 'size' : 'length';
      this.assert(len == n, `expected #{this} to have a ${what} of #{exp} but got #{act}`, `expected #{this} to not have a ${what} of #{act}`, n, len);
    },
    function () {
      flag(this, 'doLength', true);
    },
  );

  function equal(val, msg) {
    if (msg) flag(this, 'message', msg);
    if (flag(this, 'deep')) return eql.call(this, val);
    this.assert(val === flag(this, 'object'), 'expected #{this} to equal #{exp}', 'expected #{this} to not equal #{exp}', val, flag(this, 'object'));
  }
  function eql(val, msg) {
    if (msg) flag(this, 'message', msg);
    this.assert(deepEqual(flag(this, 'object'), val), 'expected #{this} to deeply equal #{exp}', 'expected #{this} to not deeply equal #{exp}', val, flag(this, 'object'));
  }
  addMethod(['equal', 'equals', 'eq'], equal);
  addMethod(['eql', 'eqls'], eql);

  function comparison(name, test, word, negWord) {
    return function (n, msg) {
      if (msg) flag(this, 'message', msg);
      const obj = flag(this, 'object');
      if (flag(this, 'doLength')) {
        const len = sizeOf(obj);
        this.assert(test(len, n), `expected #{this} to have a length ${word} #{exp} but got #{act}`, `expected #{this} to not have a length ${word} #{exp}`, n, len);
        return;
      }
      if (typeof n !== 'number' && !(n instanceof Date)) throw new AssertionError(`the argument to ${name} must be a number`);
      if (typeof obj !== 'number' && !(obj instanceof Date)) throw new AssertionError(`expected ${objDisplay(obj)} to be a number or a date`);
      this.assert(test(obj, n), `expected #{this} to be ${word} #{exp}`, `expected #{this} to be ${negWord} #{exp}`, n);
    };
  }
  addMethod(['above', 'gt', 'greaterThan'], comparison('above', (a, b) => a > b, 'above', 'at most'));
  addMethod(['least', 'gte', 'greaterThanOrEqual'], comparison('least', (a, b) => a >= b, 'at least', 'below'));
  addMethod(['below', 'lt', 'lessThan'], comparison('below', (a, b) => a < b, 'below', 'at least'));
  addMethod(['most', 'lte', 'lessThanOrEqual'], comparison('most', (a, b) => a <= b, 'at most', 'above'));
  addMethod('within', function (start, finish, msg) {
    if (msg) flag(this, 'message', msg);
    const obj = flag(this, 'object');
    const range = `${start}..${finish}`;
    if (flag(this, 'doLength')) {
      const len = sizeOf(obj);
      this.assert(len >= start && len <= finish, `expected #{this} to have a length within ${range}`, `expected #{this} to not have a length within ${range}`);
    } else {
      this.assert(obj >= start && obj <= finish, `expected #{this} to be within ${range}`, `expected #{this} to not be within ${range}`);
    }
  });
  addMethod(['closeTo', 'approximately'], function (expected, delta, msg) {
    if (msg) flag(this, 'message', msg);
    const obj = flag(this, 'object');
    this.assert(Math.abs(obj - expected) <= delta, `expected #{this} to be close to ${expected} +/- ${delta}`, `expected #{this} not to be close to ${expected} +/- ${delta}`);
  });

  addMethod('property', function (name, ...rest) {
    const hasValue = rest.length > 0;
    const val = rest[0];
    if (rest.length > 1 && rest[1]) flag(this, 'message', rest[1]);
    const obj = flag(this, 'object');
    const nested = flag(this, 'nested');
    const own = flag(this, 'own');
    const deep = flag(this, 'deep');
    const negate = flag(this, 'negate');
    let has;
    let value;
    if (nested) {
      const info = pathInfo(obj, name);
      has = info.exists;
      value = info.value;
    } else if (own) {
      has = hasOwn(obj, name);
      value = obj == null ? undefined : obj[name];
    } else {
      has = obj != null && name in Object(obj);
      value = obj == null ? undefined : obj[name];
    }
    const desc = (deep ? 'deep ' : '') + (own ? 'own ' : '') + (nested ? 'nested ' : '');
    const label = typeof name === 'string' ? quote(name) : String(name);
    if (!negate || !hasValue) this.assert(has, `expected #{this} to have ${desc}property ${label}`, `expected #{this} to not have ${desc}property ${label}`);
    if (hasValue) {
      this.assert(has && (deep ? deepEqual(val, value) : val === value), `expected #{this} to have ${desc}property ${label} of #{exp}, but got #{act}`, `expected #{this} to not have ${desc}property ${label} of #{act}`, val, value);
    }
    flag(this, 'object', value);
  });
  addMethod(['ownProperty', 'haveOwnProperty'], function (name, ...rest) {
    flag(this, 'own', true);
    return Assertion.prototype.property.call(this, name, ...rest);
  });
  addMethod('nestedProperty', function (name, ...rest) {
    flag(this, 'nested', true);
    return Assertion.prototype.property.call(this, name, ...rest);
  });

  addMethod(['keys', 'key'], function (...args) {
    const obj = flag(this, 'object');
    let keys = args.length === 1 && Array.isArray(args[0]) ? args[0] : args.length === 1 && typeOf(args[0]) === 'object' ? Object.keys(args[0]) : args;
    keys = keys.map(String);
    if (!keys.length) throw new Error('keys required');
    const actual = obj instanceof Map || obj instanceof Set ? [...obj.keys()].map(String) : Object.keys(obj == null ? {} : obj);
    const any = flag(this, 'any');
    const contains = flag(this, 'contains');
    let ok;
    if (any) ok = keys.some((k) => actual.includes(k));
    else {
      ok = keys.every((k) => actual.includes(k));
      if (!contains) ok = ok && keys.length === actual.length;
    }
    const joined = keys.length > 1 ? keys.slice(0, -1).map(quote).join(', ') + (any ? ', or ' : ', and ') + quote(keys[keys.length - 1]) : quote(keys[0]);
    const str = `${contains ? 'contain' : 'have'} ${keys.length > 1 ? 'keys' : 'key'} ${joined}`;
    this.assert(ok, `expected #{this} to ${str}`, `expected #{this} to not ${str}`, keys, actual);
  });

  addMethod('members', function (subset, msg) {
    if (msg) flag(this, 'message', msg);
    const obj = flag(this, 'object');
    if (!Array.isArray(obj) || !Array.isArray(subset)) throw new AssertionError('members() needs two arrays');
    const cmp = flag(this, 'deep') ? deepEqual : (a, b) => a === b || (a !== a && b !== b);
    const contains = flag(this, 'contains');
    let ok;
    if (flag(this, 'ordered')) ok = (contains ? subset.length <= obj.length : subset.length === obj.length) && subset.every((s, i) => cmp(obj[i], s));
    else {
      ok = subset.every((s) => obj.some((o) => cmp(o, s)));
      if (!contains) ok = ok && obj.length === subset.length && obj.every((o) => subset.some((s) => cmp(o, s)));
    }
    const what = contains ? 'be a superset of' : 'have the same members as';
    this.assert(ok, `expected #{this} to ${what} #{exp}`, `expected #{this} to not ${what} #{exp}`, subset, obj);
  });

  addMethod(['match', 'matches'], function (re, msg) {
    if (msg) flag(this, 'message', msg);
    if (!(re instanceof RegExp)) re = new RegExp(re);
    this.assert(re.exec(String(flag(this, 'object'))), `expected #{this} to match ${re}`, `expected #{this} not to match ${re}`);
  });
  addMethod('string', function (str, msg) {
    if (msg) flag(this, 'message', msg);
    this.assert(String(flag(this, 'object')).indexOf(str) !== -1, `expected #{this} to contain ${quote(String(str))}`, `expected #{this} to not contain ${quote(String(str))}`);
  });
  addMethod('oneOf', function (list, msg) {
    if (msg) flag(this, 'message', msg);
    const obj = flag(this, 'object');
    const deep = flag(this, 'deep');
    this.assert(Array.isArray(list) && list.some((v) => (deep ? deepEqual(v, obj) : v === obj)), 'expected #{this} to be one of #{exp}', 'expected #{this} to not be one of #{exp}', list, obj);
  });
  addMethod(['instanceOf', 'instanceof'], function (ctor, msg) {
    if (msg) flag(this, 'message', msg);
    const name = (ctor && ctor.name) || 'an unnamed constructor';
    this.assert(flag(this, 'object') instanceof ctor, `expected #{this} to be an instance of ${name}`, `expected #{this} to not be an instance of ${name}`);
  });
  addMethod('satisfy', function (fn, msg) {
    if (msg) flag(this, 'message', msg);
    this.assert(fn(flag(this, 'object')), `expected #{this} to satisfy ${objDisplay(fn)}`, `expected #{this} to not satisfy ${objDisplay(fn)}`);
  });
  addMethod(['throw', 'throws', 'Throw'], function (errLike, errMsg, msg) {
    if (msg) flag(this, 'message', msg);
    const fn = flag(this, 'object');
    if (typeof fn !== 'function') throw new AssertionError(`expected ${objDisplay(fn)} to be a function`);
    if (typeof errLike === 'string' || errLike instanceof RegExp) {
      errMsg = errLike;
      errLike = undefined;
    }
    let threw = false;
    let caught;
    try {
      fn();
    } catch (e) {
      threw = true;
      caught = e;
    }
    let ok = threw;
    if (ok && typeof errLike === 'function') ok = caught instanceof errLike;
    if (ok && errLike instanceof Error) ok = caught === errLike;
    if (ok && errMsg !== undefined) {
      const m = caught && caught.message !== undefined ? String(caught.message) : String(caught);
      ok = errMsg instanceof RegExp ? errMsg.test(m) : m.includes(errMsg);
    }
    const what = [typeof errLike === 'function' ? errLike.name : errLike ? objDisplay(errLike) : 'an error', errMsg !== undefined ? `including ${errMsg instanceof RegExp ? errMsg : quote(String(errMsg))}` : ''].filter(Boolean).join(' ');
    this.assert(ok, `expected #{this} to throw ${what}${threw ? ' but #{act} was thrown' : ''}`, `expected #{this} to not throw ${what} but #{act} was thrown`, errLike, caught);
    flag(this, 'object', caught);
  });

  addProperty('true', function () {
    this.assert(flag(this, 'object') === true, 'expected #{this} to be true', 'expected #{this} to be false');
  });
  addProperty('false', function () {
    this.assert(flag(this, 'object') === false, 'expected #{this} to be false', 'expected #{this} to be true');
  });
  addProperty('null', function () {
    this.assert(flag(this, 'object') === null, 'expected #{this} to be null', 'expected #{this} not to be null');
  });
  addProperty('undefined', function () {
    this.assert(flag(this, 'object') === undefined, 'expected #{this} to be undefined', 'expected #{this} not to be undefined');
  });
  addProperty('NaN', function () {
    this.assert(Number.isNaN(flag(this, 'object')), 'expected #{this} to be NaN', 'expected #{this} not to be NaN');
  });
  addProperty('finite', function () {
    this.assert(Number.isFinite(flag(this, 'object')), 'expected #{this} to be a finite number', 'expected #{this} to not be a finite number');
  });
  for (const name of ['exist', 'exists']) addProperty(name, function () {
    const o = flag(this, 'object');
    this.assert(o !== null && o !== undefined, 'expected #{this} to exist', 'expected #{this} to not exist');
  });
  addProperty('empty', function () {
    const o = flag(this, 'object');
    let n;
    switch (typeOf(o)) {
      case 'string':
      case 'array':
        n = o.length;
        break;
      case 'map':
      case 'set':
        n = o.size;
        break;
      case 'object':
        n = Object.keys(o).length;
        break;
      default:
        if (ArrayBuffer.isView(o)) n = o.length;
        else throw new AssertionError(`.empty was passed non-string primitive ${objDisplay(o)}`);
    }
    this.assert(n === 0, 'expected #{this} to be empty', 'expected #{this} not to be empty');
  });

  // Response assertions (chai-postman): `pm.response.to.*` and `pm.expect(pm.response).to.*`.
  function needResponse(a, what) {
    const r = flag(a, 'object');
    if (!isResponse(r)) throw new AssertionError(`.${what} can only be used on pm.response (got ${objDisplay(r)})`);
    return r;
  }
  addProperty('ok', function () {
    const o = flag(this, 'object');
    if (isResponse(o)) this.assert(o.code === 200, 'expected response code to be 200 but found #{act}', 'expected response code to not be 200', 200, o.code);
    else this.assert(o, 'expected #{this} to be truthy', 'expected #{this} to be falsy');
  });
  const statusClasses = {
    info: [(c) => c >= 100 && c < 200, '1XX'],
    success: [(c) => c >= 200 && c < 300, '2XX'],
    redirection: [(c) => c >= 300 && c < 400, '3XX'],
    clientError: [(c) => c >= 400 && c < 500, '4XX'],
    serverError: [(c) => c >= 500 && c < 600, '5XX'],
    error: [(c) => c >= 400 && c < 600, '4XX or 5XX'],
    accepted: [(c) => c === 202, '202'],
    badRequest: [(c) => c === 400, '400'],
    unauthorized: [(c) => c === 401, '401'],
    unauthorised: [(c) => c === 401, '401'],
    forbidden: [(c) => c === 403, '403'],
    notFound: [(c) => c === 404, '404'],
    notAcceptable: [(c) => c === 406, '406'],
    rateLimited: [(c) => c === 429, '429'],
  };
  for (const [name, [test, label]] of Object.entries(statusClasses)) {
    addProperty(name, function () {
      const r = needResponse(this, name);
      this.assert(test(r.code), `expected response code to be ${label} but found #{act}`, `expected response code to not be ${label} but found #{act}`, label, r.code);
    });
  }
  addProperty('withBody', function () {
    const r = needResponse(this, 'withBody');
    this.assert(r.text().length > 0, 'expected response to have content in body', 'expected response to not have content in body');
  });
  addProperty('json', function () {
    const r = needResponse(this, 'json');
    let err = null;
    try {
      parseJson(r.text());
    } catch (e) {
      err = e;
    }
    this.assert(!err, `expected response body to be a valid json but got error ${err && err.message}`, 'expected response body not to be a valid json');
  });
  addMethod('status', function (expected) {
    const r = needResponse(this, 'status');
    if (typeof expected === 'string' && !/^\d+$/.test(expected)) {
      this.assert(r.status === expected, 'expected response to have status reason #{exp} but got #{act}', 'expected response to not have status reason #{exp}', expected, r.status);
    } else {
      this.assert(r.code === Number(expected), 'expected response to have status code #{exp} but got #{act}', 'expected response to not have status code #{exp}', Number(expected), r.code);
    }
  });
  addMethod('header', function (name, ...rest) {
    const r = needResponse(this, 'header');
    const has = r.headers.has(name);
    if (!rest.length || !flag(this, 'negate')) this.assert(has, `expected response to have header with key ${quote(String(name))}`, `expected response to not have header with key ${quote(String(name))}`);
    if (rest.length) {
      const actual = r.headers.get(name);
      this.assert(has && String(actual) === String(rest[0]), `expected ${quote(String(name))} response header to be #{exp} but got #{act}`, `expected ${quote(String(name))} response header to not be #{exp}`, rest[0], actual);
    }
  });
  addMethod('body', function (...args) {
    const r = needResponse(this, 'body');
    const text = r.text();
    if (!args.length) return this.assert(text.length > 0, 'expected response to have content in body', 'expected response to not have content in body');
    const exp = args[0];
    if (exp instanceof RegExp) return this.assert(exp.test(text), `expected response body text to match ${exp}`, `expected response body text to not match ${exp}`);
    if (typeof exp === 'string') return this.assert(text === exp, 'expected response body to equal #{exp} but got #{act}', 'expected response body to not equal #{exp}', exp, text);
    let json;
    try {
      json = parseJson(text);
    } catch {
      return this.assert(false, 'expected response body to be valid json', 'expected response body to not be valid json');
    }
    this.assert(deepEqual(json, exp), 'expected response body json to equal #{exp} but got #{act}', 'expected response body json to not equal #{exp}', exp, json);
  });
  addMethod('jsonBody', function (...args) {
    const r = needResponse(this, 'jsonBody');
    let json;
    try {
      json = parseJson(r.text());
    } catch (e) {
      return this.assert(false, `expected response body to be a valid json but got error ${e.message}`, 'expected response body not to be a valid json');
    }
    if (!args.length) return this.assert(true, '', 'expected response body not to be a valid json');
    if (typeof args[0] === 'object' && args[0] !== null) return this.assert(deepEqual(json, args[0]), 'expected response body json to equal #{exp} but got #{act}', 'expected response body json to not equal #{exp}', args[0], json);
    const info = pathInfo(json, args[0]);
    if (args.length < 2) return this.assert(info.exists, `expected response body json to have property ${quote(String(args[0]))}`, `expected response body json to not have property ${quote(String(args[0]))}`);
    this.assert(info.exists && deepEqual(info.value, args[1]), `expected response body json at ${quote(String(args[0]))} to contain #{exp} but got #{act}`, `expected response body json at ${quote(String(args[0]))} to not contain #{exp}`, args[1], info.value);
  });
  addMethod('jsonSchema', function (schema) {
    const o = flag(this, 'object');
    let data = o;
    if (isResponse(o)) {
      try {
        data = parseJson(o.text());
      } catch (e) {
        return this.assert(false, `expected response body to be a valid json but got error ${e.message}`, 'expected response body not to be a valid json');
      }
    }
    const err = validateSchema(data, schema);
    this.assert(!err, `expected data to satisfy schema but found following errors: \n${err && `${err.path} ${err.message}`}`, 'expected data to not satisfy schema');
  });

  function expect(value, msg) {
    return new Assertion(value, msg);
  }
  expect.fail = function (actual, expected, message) {
    if (arguments.length < 2) throw new AssertionError(actual === undefined ? 'expect.fail()' : String(actual));
    throw new AssertionError(message || `expected ${objDisplay(actual)} to equal ${objDisplay(expected)}`, { actual, expected });
  };

  // ---------------------------------------------------------------- property lists, URL

  class PropertyList {
    constructor(items, caseInsensitive) {
      Object.defineProperty(this, '_items', { value: items, writable: true });
      Object.defineProperty(this, '_ci', { value: caseInsensitive });
    }
    _eq(a, b) {
      return this._ci ? String(a).toLowerCase() === String(b).toLowerCase() : String(a) === String(b);
    }
    _norm(item, value) {
      if (typeof item === 'string') {
        if (value !== undefined) return { key: item, value: asString(value) };
        const sep = this._ci ? item.indexOf(':') : item.indexOf('=');
        if (sep > 0) return { key: item.slice(0, sep).trim(), value: item.slice(sep + 1).trim() };
        return { key: item, value: '' };
      }
      if (!item || typeof item !== 'object') throw new TypeError('expected { key, value }');
      const key = item.key !== undefined ? item.key : item.name;
      if (key === undefined) throw new TypeError('expected { key, value }');
      return { key: String(key), value: item.value === undefined || item.value === null ? '' : asString(item.value), disabled: !!item.disabled };
    }
    _match(pred) {
      if (typeof pred === 'function') return pred;
      const key = pred && typeof pred === 'object' ? (pred.key !== undefined ? pred.key : pred.name) : pred;
      return (i) => this._eq(i.key, key);
    }
    add(item, value) {
      this._items.push(this._norm(item, value));
    }
    append(item, value) {
      this.add(item, value);
    }
    prepend(item, value) {
      this._items.unshift(this._norm(item, value));
    }
    upsert(item, value) {
      const n = this._norm(item, value);
      const existing = this._items.find((i) => this._eq(i.key, n.key));
      if (existing) {
        existing.value = n.value;
        existing.disabled = n.disabled;
      } else this._items.push(n);
    }
    remove(pred) {
      const m = this._match(pred);
      for (let i = this._items.length - 1; i >= 0; i--) if (m(this._items[i])) this._items.splice(i, 1);
    }
    clear() {
      this._items.length = 0;
    }
    one(key) {
      const found = this._items.find((i) => !i.disabled && this._eq(i.key, key));
      return found ? { ...found } : undefined;
    }
    get(key) {
      const found = this.one(key);
      return found ? found.value : undefined;
    }
    has(key, value) {
      return this._items.some((i) => !i.disabled && this._eq(i.key, key) && (value === undefined || i.value === asString(value)));
    }
    all() {
      return this._items.map((i) => ({ ...i }));
    }
    toObject() {
      const o = {};
      for (const i of this._items) if (!i.disabled) o[i.key] = i.value;
      return o;
    }
    toJSON() {
      return this.all();
    }
    count() {
      return this._items.length;
    }
    idx(n) {
      return this._items[n] ? { ...this._items[n] } : undefined;
    }
    indexOf(key) {
      return this._items.findIndex(this._match(key));
    }
    each(fn) {
      this._items.forEach((i, n) => fn(i, n));
    }
    forEach(fn) {
      this.each(fn);
    }
    map(fn) {
      return this._items.map((i, n) => fn(i, n));
    }
    filter(fn) {
      return this._items.filter((i, n) => fn(i, n));
    }
    find(fn) {
      return this._items.find((i, n) => fn(i, n));
    }
    reduce(fn, acc) {
      return this._items.reduce(fn, acc);
    }
    toString() {
      return this._items.filter((i) => !i.disabled).map((i) => (this._ci ? `${i.key}: ${i.value}` : `${i.key}=${i.value}`)).join(this._ci ? '\n' : '&');
    }
    [INSPECT]() {
      return this.toObject();
    }
  }

  class Url {
    constructor(base, query, hash) {
      Object.defineProperty(this, '_base', { value: String(base), writable: true });
      this.query = new PropertyList(query, false);
      this.hash = hash || '';
    }
    static parse(raw) {
      raw = String(raw);
      let hash = '';
      const h = raw.indexOf('#');
      if (h >= 0) {
        hash = raw.slice(h + 1);
        raw = raw.slice(0, h);
      }
      const q = raw.indexOf('?');
      const query = [];
      if (q >= 0) {
        for (const p of raw.slice(q + 1).split('&')) {
          if (!p) continue;
          const e = p.indexOf('=');
          query.push(e < 0 ? { key: decode(p), value: '' } : { key: decode(p.slice(0, e)), value: decode(p.slice(e + 1)) });
        }
        raw = raw.slice(0, q);
      }
      return new Url(raw, query, hash);
    }
    update(raw) {
      const u = raw instanceof Url ? raw : Url.parse(raw && typeof raw === 'object' && raw.raw !== undefined ? raw.raw : raw);
      this._base = u._base;
      this.query._items.splice(0, this.query._items.length, ...u.query._items.map((i) => ({ ...i })));
      this.hash = u.hash;
    }
    _parts() {
      const m = /^(?:([A-Za-z][\w+.-]*):\/\/)?([^/?#]*)(.*)$/.exec(this._base) || [];
      let authority = m[2] || '';
      const at = authority.lastIndexOf('@');
      if (at >= 0) authority = authority.slice(at + 1);
      const pm_ = /^(\[[^\]]*\]|[^:]*)(?::(.*))?$/.exec(authority) || [];
      return { protocol: m[1], host: pm_[1] || '', port: pm_[2], path: m[3] || '' };
    }
    get protocol() {
      return this._parts().protocol;
    }
    get host() {
      const h = this._parts().host;
      return h ? h.split('.') : [];
    }
    get port() {
      return this._parts().port;
    }
    get path() {
      return this._parts().path.split('/').filter(Boolean);
    }
    getHost() {
      return this._parts().host;
    }
    getRemote(forcePort) {
      const p = this._parts();
      return p.port || forcePort ? `${p.host}:${p.port || (p.protocol === 'https' ? 443 : 80)}` : p.host;
    }
    getPath(unresolved) {
      const p = this._parts().path;
      return p ? (p.startsWith('/') ? p : '/' + p) : '/';
    }
    getQueryString() {
      return this.query._items.filter((i) => !i.disabled).map((i) => `${i.key}=${i.value}`).join('&');
    }
    getPathWithQuery() {
      const q = this.getQueryString();
      return this.getPath() + (q ? '?' + q : '');
    }
    addQueryParams(params) {
      if (typeof params === 'string') params = Url.parse('?' + params).query._items;
      for (const p of [].concat(params)) this.query.add(p);
    }
    removeQueryParams(params) {
      for (const p of [].concat(params)) this.query.remove(p);
    }
    toString() {
      const q = this.getQueryString();
      return this._base + (q ? '?' + q : '') + (this.hash ? '#' + this.hash : '');
    }
    toJSON() {
      return { raw: this.toString(), protocol: this.protocol, host: this.host, port: this.port, path: this.path, query: this.query.all(), hash: this.hash || undefined };
    }
    [INSPECT]() {
      return this.toString();
    }
  }

  // ---------------------------------------------------------------- variables

  const RT = Object.assign(Object.create(null), I.vars.runtime);
  const DATA = Object.freeze(Object.assign(Object.create(null), I.vars.data));
  const ENV = I.vars.environment;
  const COLL = I.vars.collection;
  const SECRETS = I.secrets || {};
  const changes = [];

  function replaceIn(s, get, depth = 0) {
    if (typeof s !== 'string') return s;
    return s.replace(/\{\{\s*([^{}]*?)\s*\}\}/g, (m, key) => {
      if (!key || key.startsWith('secret.') || key.startsWith('$env.')) return m; // resolved when sending
      if (key.startsWith('$')) {
        const v = N.dynamic(key);
        return v === undefined ? m : v;
      }
      const v = get(key);
      if (v === undefined) return m;
      const sv = asString(v);
      return depth < 8 ? replaceIn(sv, get, depth + 1) : sv;
    });
  }

  function scope(name, layers, extra) {
    const get = (k) => {
      k = String(k);
      if (k in RT) return RT[k];
      for (const l of layers) if (hasOwn(l, k)) return l[k];
      return undefined;
    };
    const api = {
      get,
      has: (k) => get(k) !== undefined,
      set(k, v) {
        if (k === undefined || k === null || k === '') throw new TypeError(`pm.${name}.set: a variable name is required`);
        k = String(k);
        RT[k] = v;
        changes.push({ scope: name, name: k, value: asString(v) });
      },
      unset(k) {
        k = String(k);
        delete RT[k];
        changes.push({ scope: name, name: k, value: null });
      },
      clear() {
        for (const k of Object.keys(api.toObject())) if (k in RT) api.unset(k);
      },
      toObject() {
        const o = {};
        for (let i = layers.length - 1; i >= 0; i--) Object.assign(o, layers[i]);
        for (const k of Object.keys(RT)) o[k] = RT[k];
        return o;
      },
      toJSON() {
        return api.toObject();
      },
      replaceIn: (s) => replaceIn(s, get),
      [INSPECT]: () => api.toObject(),
    };
    return Object.assign(api, extra);
  }

  const variables = scope('variables', [DATA, ENV, COLL]);
  const environment = scope('environment', [ENV], { name: I.info.environmentName || undefined });
  const collectionVariables = scope('collectionVariables', [COLL]);
  const globals = scope('globals', []);
  const iterationData = {
    get: (k) => DATA[String(k)],
    has: (k) => hasOwn(DATA, String(k)),
    toObject: () => ({ ...DATA }),
    toJSON: () => ({ ...DATA }),
    unset: () => warn('pm.iterationData is read-only'),
    set: () => warn('pm.iterationData is read-only'),
    replaceIn: (s) => replaceIn(s, (k) => DATA[k]),
    [INSPECT]: () => ({ ...DATA }),
  };

  // ---------------------------------------------------------------- request

  const R = I.request;
  let rawBody = R.body === null || R.body === undefined ? null : R.body;
  const body = {
    mode: rawBody === null ? 'none' : 'raw',
    get raw() {
      return rawBody === null ? '' : rawBody;
    },
    set raw(v) {
      rawBody = typeof v === 'string' ? v : asString(v);
      this.mode = 'raw';
    },
    update(v) {
      if (v && typeof v === 'object' && !Array.isArray(v)) {
        if (v.mode && v.mode !== 'raw') unsupported(`pm.request.body.update({ mode: '${v.mode}' })`, 'only raw bodies can be edited');
        this.raw = v.raw === undefined ? '' : v.raw;
      } else this.raw = v === undefined || v === null ? '' : v;
    },
    isEmpty() {
      return !rawBody;
    },
    toString() {
      return rawBody === null ? '' : rawBody;
    },
    toJSON() {
      return { mode: this.mode, raw: this.raw };
    },
    [INSPECT]() {
      return this.toJSON();
    },
  };

  let url = Url.parse(R.url);
  for (const [key, value] of R.query || []) url.query.add({ key, value });
  const reqHeaders = new PropertyList(
    (R.headers || []).map(([key, value]) => ({ key, value })),
    true,
  );
  const request = {
    method: R.method || 'GET',
    get url() {
      return url;
    },
    set url(v) {
      url = v instanceof Url ? v : Url.parse(v && typeof v === 'object' && v.raw !== undefined ? v.raw : String(v));
    },
    get headers() {
      return reqHeaders;
    },
    get body() {
      return body;
    },
    set body(v) {
      body.update(v);
    },
    get auth() {
      return undefined;
    },
    name: I.info.requestName,
    id: I.info.requestId,
    addHeader: (h, v) => reqHeaders.add(h, v),
    upsertHeader: (h, v) => reqHeaders.upsert(h, v),
    removeHeader: (h) => reqHeaders.remove(h),
    getHeaders: () => reqHeaders.toObject(),
    addQueryParams: (p) => url.addQueryParams(p),
    removeQueryParams: (p) => url.removeQueryParams(p),
    toJSON() {
      return { name: this.name, method: this.method, url: url.toString(), header: reqHeaders.all(), body: body.toJSON() };
    },
    [INSPECT]() {
      return this.toJSON();
    },
  };

  function requestOut() {
    const items = (list) => list._items.filter((i) => !i.disabled).map((i) => [String(i.key), i.value === undefined || i.value === null ? '' : String(i.value)]);
    return {
      method: String(request.method || 'GET').toUpperCase(),
      url: url._base + (url.hash ? '#' + url.hash : ''),
      query: items(url.query),
      headers: items(reqHeaders),
      body: rawBody === null || (R.body === null && rawBody === '') ? null : String(rawBody),
    };
  }

  // ---------------------------------------------------------------- response

  class Response {
    constructor(r, text) {
      this.code = r.code;
      this.status = r.status;
      this.responseTime = r.responseTime;
      this.responseSize = r.size;
      Object.defineProperty(this, 'headers', { value: new PropertyList(r.headers.map(([key, value]) => ({ key, value })), true), enumerable: true });
      Object.defineProperty(this, '_text', { value: text });
    }
    reason() {
      return this.status;
    }
    text() {
      return this._text;
    }
    json() {
      try {
        return parseJson(this._text);
      } catch (e) {
        const err = new SyntaxError(`JSONError: ${e.message}`);
        err.name = 'JSONError';
        throw err;
      }
    }
    size() {
      const header = this.headers._items.reduce((n, h) => n + h.key.length + String(h.value).length + 4, 0);
      return { body: this.responseSize, header, total: this.responseSize + header };
    }
    get to() {
      return new Assertion(this);
    }
    get cookies() {
      return cookies;
    }
    toJSON() {
      return { code: this.code, status: this.status, header: this.headers.all(), body: this._text, responseTime: this.responseTime, responseSize: this.responseSize };
    }
    [INSPECT]() {
      return { code: this.code, status: this.status, responseTime: this.responseTime, responseSize: this.responseSize };
    }
  }
  const response = PRE || !I.response ? undefined : new Response(I.response, BODY);

  const cookieMap = new Map();
  if (response) {
    for (const h of response.headers._items) {
      if (h.key.toLowerCase() !== 'set-cookie') continue;
      const first = String(h.value).split(';')[0];
      const e = first.indexOf('=');
      if (e > 0) cookieMap.set(first.slice(0, e).trim(), first.slice(e + 1).trim());
    }
  }
  const cookies = {
    get: (name) => cookieMap.get(name),
    has: (name) => cookieMap.has(name),
    toObject: () => Object.fromEntries(cookieMap),
    all: () => [...cookieMap].map(([name, value]) => ({ name, value })),
    jar: () => unsupported('pm.cookies.jar()'),
    [INSPECT]: () => Object.fromEntries(cookieMap),
  };

  // ---------------------------------------------------------------- tests

  const tests = [];
  function test(name, fn) {
    const t = { name: String(name), passed: false, skipped: false, message: null, done: false };
    tests.push(t);
    const pass = () => {
      if (!t.done) Object.assign(t, { done: true, passed: true });
    };
    const fail = (e) => {
      if (!t.done) Object.assign(t, { done: true, passed: false, message: errorText(e) });
    };
    if (typeof fn !== 'function') {
      Object.assign(t, { done: true, passed: true, skipped: true });
      return;
    }
    try {
      if (fn.length > 0) {
        fn((err) => (err ? fail(err) : pass()));
      } else {
        const r = fn();
        if (r && typeof r.then === 'function') r.then(pass, fail);
        else pass();
      }
    } catch (e) {
      fail(e);
    }
  }
  test.skip = (name) => {
    tests.push({ name: String(name), passed: true, skipped: true, message: null, done: true });
  };

  // ---------------------------------------------------------------- crypto & encoding

  class WordArray {
    constructor(hex = '') {
      Object.defineProperty(this, '_hex', { value: hex, writable: true });
    }
    static create(words, sigBytes) {
      if (words instanceof ArrayBuffer) words = new Uint8Array(words);
      if (ArrayBuffer.isView(words)) return new WordArray(bytesToHex(new Uint8Array(words.buffer, words.byteOffset, words.byteLength)));
      words = words || [];
      let hex = words.map((w) => (w >>> 0).toString(16).padStart(8, '0')).join('');
      if (sigBytes !== undefined) hex = hex.slice(0, sigBytes * 2).padEnd(sigBytes * 2, '0');
      return new WordArray(hex);
    }
    static random(n) {
      return new WordArray(N.randomHex(n));
    }
    get sigBytes() {
      return this._hex.length >> 1;
    }
    set sigBytes(n) {
      this._hex = this._hex.slice(0, Math.max(0, n) * 2).padEnd(Math.max(0, n) * 2, '0');
    }
    get words() {
      const w = [];
      for (let i = 0; i < this._hex.length; i += 8) w.push(parseInt(this._hex.slice(i, i + 8).padEnd(8, '0'), 16) | 0);
      return w;
    }
    toString(encoder) {
      return (encoder || Hex).stringify(this);
    }
    concat(other) {
      this._hex += toWordArray(other)._hex;
      return this;
    }
    clone() {
      return new WordArray(this._hex);
    }
    clamp() {
      return this;
    }
    [INSPECT]() {
      return { words: this.words, sigBytes: this.sigBytes };
    }
  }
  function toWordArray(x) {
    if (x instanceof WordArray) return x;
    if (x === undefined || x === null) return new WordArray('');
    if (x instanceof ArrayBuffer || ArrayBuffer.isView(x)) return WordArray.create(x);
    return Utf8.parse(String(x));
  }
  const Hex = {
    stringify: (wa) => toWordArray(wa)._hex,
    parse: (s) => {
      s = String(s).toLowerCase();
      if (s.length % 2 || /[^0-9a-f]/.test(s)) throw new Error('CryptoJS.enc.Hex.parse: invalid hex string');
      return new WordArray(s);
    },
  };
  const Latin1 = { stringify: (wa) => hexToLatin1(wa._hex), parse: (s) => new WordArray(latin1ToHex(String(s))) };
  const Utf8 = { stringify: (wa) => N.hexToUtf8(wa._hex), parse: (s) => new WordArray(N.utf8ToHex(String(s))) };
  const Base64 = { stringify: (wa) => N.hexToB64(wa._hex, false), parse: (s) => new WordArray(N.b64ToHex(String(s))) };
  const Base64url = { stringify: (wa) => N.hexToB64(wa._hex, true), parse: (s) => new WordArray(N.b64ToHex(String(s))) };
  const HASHES = { MD5: 'md5', SHA1: 'sha1', SHA224: 'sha224', SHA256: 'sha256', SHA384: 'sha384', SHA512: 'sha512' };
  const hasher = (alg) => (msg) => new WordArray(N.hash(alg, toWordArray(msg)._hex));
  const hmacFn = (alg) => (msg, key) => new WordArray(N.hmac(alg, toWordArray(key)._hex, toWordArray(msg)._hex));
  const progressive = (alg, key) => {
    let acc = '';
    return {
      update(m) {
        acc += toWordArray(m)._hex;
        return this;
      },
      finalize(m) {
        if (m !== undefined) this.update(m);
        const out = key === undefined ? N.hash(alg, acc) : N.hmac(alg, key, acc);
        acc = '';
        return new WordArray(out);
      },
      reset() {
        acc = '';
        return this;
      },
    };
  };
  const CryptoJS = {
    lib: { WordArray },
    enc: { Hex, Latin1, Utf8, Base64, Base64url },
    algo: {
      HMAC: {
        create: (algo, key) => {
          if (!algo || !algo._alg) unsupported('CryptoJS.algo.HMAC with this hasher');
          return progressive(algo._alg, toWordArray(key)._hex);
        },
      },
    },
  };
  for (const [name, alg] of Object.entries(HASHES)) {
    CryptoJS[name] = hasher(alg);
    CryptoJS['Hmac' + name] = hmacFn(alg);
    CryptoJS.algo[name] = { _alg: alg, create: () => progressive(alg) };
  }
  for (const name of ['AES', 'DES', 'TripleDES', 'RC4', 'RC4Drop', 'Rabbit', 'RabbitLegacy', 'SHA3', 'RIPEMD160', 'HmacSHA3', 'HmacRIPEMD160', 'PBKDF2', 'EvpKDF']) {
    Object.defineProperty(CryptoJS, name, {
      get: () => unsupported(`CryptoJS.${name}`, 'available: MD5, SHA1, SHA224, SHA256, SHA384, SHA512 and their Hmac* variants'),
    });
  }

  class Buffer extends Uint8Array {
    static from(v, enc) {
      let bytes;
      if (typeof v === 'string') {
        switch (String(enc || 'utf8').toLowerCase()) {
          case 'utf8':
          case 'utf-8':
            bytes = hexToBytes(N.utf8ToHex(v));
            break;
          case 'hex':
            bytes = hexToBytes(Hex.parse(v)._hex);
            break;
          case 'base64':
          case 'base64url':
            bytes = hexToBytes(N.b64ToHex(v));
            break;
          case 'latin1':
          case 'binary':
          case 'ascii':
            bytes = hexToBytes(latin1ToHex(v));
            break;
          default:
            throw new TypeError(`Unknown encoding: ${enc}`);
        }
      } else if (v instanceof ArrayBuffer) bytes = new Uint8Array(v);
      else if (ArrayBuffer.isView(v)) bytes = new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
      else if (Array.isArray(v)) bytes = v;
      else if (v && v.type === 'Buffer' && Array.isArray(v.data)) bytes = v.data;
      else throw new TypeError('Buffer.from: unsupported input');
      const b = new Buffer(bytes.length);
      b.set(bytes);
      return b;
    }
    static alloc(n, fill = 0) {
      return new Buffer(n).fill(typeof fill === 'number' ? fill : 0);
    }
    static isBuffer(b) {
      return b instanceof Buffer;
    }
    static byteLength(s, enc) {
      return typeof s === 'string' ? Buffer.from(s, enc).length : s.byteLength;
    }
    static concat(list) {
      const total = list.reduce((n, b) => n + b.length, 0);
      const out = new Buffer(total);
      let at = 0;
      for (const b of list) {
        out.set(b, at);
        at += b.length;
      }
      return out;
    }
    toString(enc = 'utf8') {
      const hex = bytesToHex(this);
      switch (String(enc).toLowerCase()) {
        case 'utf8':
        case 'utf-8':
          return N.hexToUtf8(hex);
        case 'hex':
          return hex;
        case 'base64':
          return N.hexToB64(hex, false);
        case 'base64url':
          return N.hexToB64(hex, true);
        case 'latin1':
        case 'binary':
        case 'ascii':
          return hexToLatin1(hex);
        default:
          throw new TypeError(`Unknown encoding: ${enc}`);
      }
    }
    equals(other) {
      return this.length === other.length && this.every((v, i) => v === other[i]);
    }
    toJSON() {
      return { type: 'Buffer', data: Array.from(this) };
    }
  }

  class TextEncoder {
    get encoding() {
      return 'utf-8';
    }
    encode(s = '') {
      return hexToBytes(N.utf8ToHex(String(s)));
    }
  }
  class TextDecoder {
    constructor(label = 'utf-8') {
      if (!/^utf-?8$/i.test(label)) unsupported(`TextDecoder('${label}')`, 'only UTF-8 is available');
    }
    get encoding() {
      return 'utf-8';
    }
    decode(b) {
      if (!b) return '';
      const bytes = b instanceof ArrayBuffer ? new Uint8Array(b) : new Uint8Array(b.buffer, b.byteOffset, b.byteLength);
      return N.hexToUtf8(bytesToHex(bytes));
    }
  }

  const crypto = {
    randomUUID: () => N.uuid(),
    getRandomValues(arr) {
      if (!ArrayBuffer.isView(arr)) throw new TypeError('crypto.getRandomValues: expected a typed array');
      const bytes = hexToBytes(N.randomHex(arr.byteLength));
      new Uint8Array(arr.buffer, arr.byteOffset, arr.byteLength).set(bytes);
      return arr;
    },
    get subtle() {
      return unsupported('crypto.subtle', 'use CryptoJS');
    },
  };

  // ---------------------------------------------------------------- timers (virtual time)

  const timers = new Map();
  let timerSeq = 0;
  let virtualNow = 0;
  let timerRuns = 0;
  function addTimer(fn, ms, args, repeat) {
    if (typeof fn !== 'function') throw new TypeError('timer callback must be a function');
    const delay = Math.max(0, Number(ms) || 0);
    const id = ++timerSeq;
    timers.set(id, { at: virtualNow + delay, seq: id, fn, args, interval: repeat ? Math.max(1, delay) : null });
    return id;
  }
  function nextTimer() {
    if (!timers.size) return false;
    if (++timerRuns > 10000) {
      timers.clear();
      warn('stopped running timers after 10000 callbacks (is a setInterval never cleared?)');
      return false;
    }
    let bestId = null;
    let best = null;
    for (const [id, t] of timers) if (!best || t.at < best.at || (t.at === best.at && t.seq < best.seq)) [bestId, best] = [id, t];
    virtualNow = best.at;
    if (best.interval !== null) {
      best.at += best.interval;
      best.seq = ++timerSeq;
    } else timers.delete(bestId);
    best.fn(...best.args);
    return true;
  }

  // ---------------------------------------------------------------- pm

  let skipRequest = false;
  const pm = {
    info: Object.freeze({
      eventName: EVENT,
      iteration: I.info.iteration,
      iterationCount: Math.max(1, I.info.iterationCount),
      requestName: I.info.requestName,
      requestId: I.info.requestId,
    }),
    variables,
    environment,
    collectionVariables,
    globals,
    iterationData,
    request,
    response,
    cookies,
    test,
    expect,
    execution: {
      skipRequest() {
        if (!PRE) return warn('pm.execution.skipRequest() only works in pre-request scripts');
        skipRequest = true;
      },
      setNextRequest: () => warn('pm.execution.setNextRequest is not supported: requests run in tree order'),
      location: Object.freeze(Object.assign(['devall', ...String(I.info.requestPath || '').split('/').filter(Boolean)], { current: 'devall' })),
    },
    secrets: {
      get: (name) => (hasOwn(SECRETS, name) ? SECRETS[name] : undefined),
      has: (name) => hasOwn(SECRETS, name),
    },
    vault: {
      get: async (name) => (hasOwn(SECRETS, name) ? SECRETS[name] : undefined),
      set: async () => unsupported('pm.vault.set', 'store secrets in the OS keychain from the environment editor'),
      unset: async () => unsupported('pm.vault.unset'),
    },
    sendRequest: () => unsupported('pm.sendRequest', 'chain requests with captures or a collection run instead'),
    visualizer: { set: () => N.log('info', 'pm.visualizer is not supported in /dev/all — the template was ignored'), clear: () => {} },
    require: (name) => unsupported(`pm.require('${name}')`, 'package libraries are not available'),
  };

  // ---------------------------------------------------------------- lodash subset

  function iteratee(p) {
    if (typeof p === 'function') return p;
    if (p === undefined || p === null) return (x) => x;
    if (Array.isArray(p)) return (x) => deepEqual(pathInfo(x, p[0]).value, p[1]);
    if (typeof p === 'object') return (x) => x != null && Object.keys(p).every((k) => deepEqual(x[k], p[k]));
    return (x) => pathInfo(x, p).value;
  }
  const coll = (c) => (Array.isArray(c) ? c : c && typeof c === 'object' ? Object.values(c) : []);
  const lodash = {
    get: (o, p, d) => {
      const v = pathInfo(o, p).value;
      return v === undefined ? d : v;
    },
    has: (o, p) => pathInfo(o, p).exists,
    set(o, p, v) {
      const parts = parsePath(p);
      let cur = o;
      parts.forEach((k, i) => {
        if (i === parts.length - 1) cur[k] = v;
        else {
          if (cur[k] === null || typeof cur[k] !== 'object') cur[k] = /^\d+$/.test(parts[i + 1]) ? [] : {};
          cur = cur[k];
        }
      });
      return o;
    },
    isEqual: (a, b) => deepEqual(a, b),
    isEmpty: (v) => v == null || (typeof v === 'string' || Array.isArray(v) ? v.length === 0 : v instanceof Map || v instanceof Set ? v.size === 0 : typeof v === 'object' ? Object.keys(v).length === 0 : true),
    isNil: (v) => v === null || v === undefined,
    isNull: (v) => v === null,
    isUndefined: (v) => v === undefined,
    isString: (v) => typeof v === 'string',
    isNumber: (v) => typeof v === 'number',
    isInteger: Number.isInteger,
    isBoolean: (v) => typeof v === 'boolean',
    isArray: Array.isArray,
    isFunction: (v) => typeof v === 'function',
    isObject: (v) => v !== null && (typeof v === 'object' || typeof v === 'function'),
    isPlainObject: (v) => typeOf(v) === 'object' && [Object.prototype, null].includes(Object.getPrototypeOf(v)),
    keys: (o) => Object.keys(o || {}),
    values: (o) => Object.values(o || {}),
    entries: (o) => Object.entries(o || {}),
    toPairs: (o) => Object.entries(o || {}),
    fromPairs: (p) => Object.fromEntries(p),
    find: (c, p) => coll(c).find(iteratee(p)),
    findIndex: (a, p) => (a || []).findIndex(iteratee(p)),
    filter: (c, p) => coll(c).filter(iteratee(p)),
    reject: (c, p) => coll(c).filter((x) => !iteratee(p)(x)),
    map: (c, p) => coll(c).map(iteratee(p)),
    forEach: (c, f) => (Array.isArray(c) ? c.forEach(f) : Object.entries(c || {}).forEach(([k, v]) => f(v, k))),
    each: (c, f) => lodash.forEach(c, f),
    some: (c, p) => coll(c).some(iteratee(p)),
    every: (c, p) => coll(c).every(iteratee(p)),
    includes: (c, v) => (typeof c === 'string' ? c.includes(v) : coll(c).includes(v)),
    reduce: (c, f, acc) => coll(c).reduce(f, acc),
    size: (c) => (c == null ? 0 : typeof c === 'string' || Array.isArray(c) ? c.length : Object.keys(c).length),
    random(a = 0, b = 1, float = false) {
      if (b === undefined || typeof b === 'boolean') [a, b, float] = [0, a, !!b];
      const lo = Math.min(a, b);
      const hi = Math.max(a, b);
      return float || !Number.isInteger(lo) || !Number.isInteger(hi) ? lo + Math.random() * (hi - lo) : lo + Math.floor(Math.random() * (hi - lo + 1));
    },
    sample: (c) => {
      const a = coll(c);
      return a[Math.floor(Math.random() * a.length)];
    },
    shuffle: (c) => {
      const a = coll(c).slice();
      for (let i = a.length - 1; i > 0; i--) {
        const j = Math.floor(Math.random() * (i + 1));
        [a[i], a[j]] = [a[j], a[i]];
      }
      return a;
    },
    uniq: (a) => [...new Set(a || [])],
    uniqBy: (a, p) => {
      const f = iteratee(p);
      const seen = new Set();
      return (a || []).filter((x) => {
        const k = f(x);
        if (seen.has(k)) return false;
        seen.add(k);
        return true;
      });
    },
    flatten: (a) => (a || []).flat(),
    flattenDeep: (a) => (a || []).flat(Infinity),
    compact: (a) => (a || []).filter(Boolean),
    chunk: (a, n = 1) => {
      const out = [];
      for (let i = 0; i < (a || []).length; i += n) out.push(a.slice(i, i + n));
      return out;
    },
    range(start, end, step) {
      if (end === undefined) [start, end] = [0, start];
      step = step === undefined ? (start < end ? 1 : -1) : step;
      const out = [];
      if (step === 0) return out;
      for (let i = start; step > 0 ? i < end : i > end; i += step) out.push(i);
      return out;
    },
    times: (n, f = (i) => i) => Array.from({ length: n }, (_, i) => f(i)),
    first: (a) => (a || [])[0],
    head: (a) => (a || [])[0],
    last: (a) => (a || [])[(a || []).length - 1],
    sortBy: (c, ...ps) => {
      const fs = ps.flat().map(iteratee);
      if (!fs.length) fs.push((x) => x);
      return coll(c)
        .slice()
        .sort((x, y) => {
          for (const f of fs) {
            const a = f(x);
            const b = f(y);
            if (a < b) return -1;
            if (a > b) return 1;
          }
          return 0;
        });
    },
    groupBy: (c, p) => coll(c).reduce((o, x) => ((o[iteratee(p)(x)] ||= []).push(x), o), {}),
    keyBy: (c, p) => coll(c).reduce((o, x) => ((o[iteratee(p)(x)] = x), o), {}),
    countBy: (c, p) => coll(c).reduce((o, x) => ((o[iteratee(p)(x)] = (o[iteratee(p)(x)] || 0) + 1), o), {}),
    pick: (o, ...ks) => Object.fromEntries(ks.flat().filter((k) => o != null && k in Object(o)).map((k) => [k, o[k]])),
    omit: (o, ...ks) => {
      const drop = new Set(ks.flat().map(String));
      return Object.fromEntries(Object.entries(o || {}).filter(([k]) => !drop.has(k)));
    },
    clone: (v) => (Array.isArray(v) ? v.slice() : v && typeof v === 'object' ? { ...v } : v),
    cloneDeep: (v) => (v === undefined ? v : parseJson(stringify(v))),
    merge(target, ...sources) {
      for (const s of sources) {
        for (const [k, v] of Object.entries(s || {})) {
          if (v && typeof v === 'object' && !Array.isArray(v) && target[k] && typeof target[k] === 'object') lodash.merge(target[k], v);
          else if (v !== undefined) target[k] = v;
        }
      }
      return target;
    },
    assign: Object.assign,
    defaults: (o, ...s) => {
      for (const src of s) for (const [k, v] of Object.entries(src || {})) if (o[k] === undefined) o[k] = v;
      return o;
    },
    capitalize: (s = '') => String(s).charAt(0).toUpperCase() + String(s).slice(1).toLowerCase(),
    upperFirst: (s = '') => String(s).charAt(0).toUpperCase() + String(s).slice(1),
    camelCase: (s = '') =>
      String(s)
        .toLowerCase()
        .replace(/[^a-z0-9]+(.)/g, (_, c) => c.toUpperCase())
        .replace(/^[^a-z0-9]+/, ''),
    kebabCase: (s = '') =>
      String(s)
        .replace(/([a-z])([A-Z])/g, '$1-$2')
        .replace(/[^A-Za-z0-9]+/g, '-')
        .replace(/^-|-$/g, '')
        .toLowerCase(),
    snakeCase: (s = '') =>
      String(s)
        .replace(/([a-z])([A-Z])/g, '$1_$2')
        .replace(/[^A-Za-z0-9]+/g, '_')
        .replace(/^_|_$/g, '')
        .toLowerCase(),
    toLower: (s = '') => String(s).toLowerCase(),
    toUpper: (s = '') => String(s).toUpperCase(),
    trim: (s = '') => String(s).trim(),
    startsWith: (s = '', t) => String(s).startsWith(t),
    endsWith: (s = '', t) => String(s).endsWith(t),
    padStart: (s = '', n, c) => String(s).padStart(n, c),
    padEnd: (s = '', n, c) => String(s).padEnd(n, c),
    repeat: (s = '', n) => String(s).repeat(n),
    split: (s = '', sep, n) => String(s).split(sep, n),
    escapeRegExp: (s = '') => String(s).replace(/[.*+?^${}()|[\]\\]/g, '\\$&'),
    sum: (a) => (a || []).reduce((n, x) => n + x, 0),
    sumBy: (a, p) => (a || []).reduce((n, x) => n + iteratee(p)(x), 0),
    min: (a) => ((a || []).length ? Math.min(...a) : undefined),
    max: (a) => ((a || []).length ? Math.max(...a) : undefined),
    mean: (a) => ((a || []).length ? a.reduce((n, x) => n + x, 0) / a.length : NaN),
    toNumber: Number,
    toString: (v) => (v == null ? '' : String(v)),
    now: () => Date.now(),
    noop: () => {},
    identity: (x) => x,
    uniqueId: (() => {
      let n = 0;
      return (prefix = '') => `${prefix}${++n}`;
    })(),
  };
  const _ = new Proxy(lodash, {
    get(t, k) {
      if (typeof k === 'symbol' || k in t) return t[k];
      throw new TypeError(`_.${String(k)} is not available in /dev/all scripts (only a lodash subset is built in)`);
    },
  });

  const tv4 = {
    error: null,
    validate(data, schema) {
      const e = validateSchema(data, schema);
      tv4.error = e ? { message: e.message, dataPath: e.path.replace(/^data/, '') } : null;
      return !e;
    },
    validateResult(data, schema) {
      const valid = tv4.validate(data, schema);
      return { valid, error: tv4.error, missing: [] };
    },
  };

  // ---------------------------------------------------------------- legacy Postman globals

  const postman = {
    setEnvironmentVariable: (k, v) => environment.set(k, v),
    getEnvironmentVariable: (k) => environment.get(k),
    clearEnvironmentVariable: (k) => environment.unset(k),
    clearEnvironmentVariables: () => environment.clear(),
    setGlobalVariable: (k, v) => globals.set(k, v),
    getGlobalVariable: (k) => globals.get(k),
    clearGlobalVariable: (k) => globals.unset(k),
    clearGlobalVariables: () => globals.clear(),
    getResponseHeader: (k) => (response ? response.headers.get(k) : undefined),
    getResponseCookie: (k) => (cookieMap.has(k) ? { name: k, value: cookieMap.get(k) } : undefined),
    setNextRequest: () => warn('postman.setNextRequest is not supported: requests run in tree order'),
  };

  // ---------------------------------------------------------------- Bruno shims

  const bodyValue = (text) => {
    try {
      return parseJson(text);
    } catch {
      return text;
    }
  };
  const bru = {
    getEnvVar: (k) => environment.get(k),
    setEnvVar: (k, v) => environment.set(k, v),
    hasEnvVar: (k) => environment.has(k),
    deleteEnvVar: (k) => environment.unset(k),
    getVar: (k) => RT[String(k)],
    setVar: (k, v) => variables.set(k, v),
    hasVar: (k) => String(k) in RT,
    deleteVar: (k) => variables.unset(k),
    deleteAllVars: () => {
      for (const k of Object.keys(RT)) variables.unset(k);
    },
    getCollectionVar: (k) => collectionVariables.get(k),
    getFolderVar: (k) => collectionVariables.get(k),
    getRequestVar: (k) => variables.get(k),
    getGlobalEnvVar: (k) => globals.get(k),
    setGlobalEnvVar: (k, v) => globals.set(k, v),
    getEnvName: () => I.info.environmentName || undefined,
    getProcessEnv: (k) => {
      warn(`bru.getProcessEnv('${k}'): the process environment is not available to scripts — use {{$env.${k}}} in request fields`);
      return undefined;
    },
    interpolate: (s) => variables.replaceIn(s),
    sleep: (ms) => new Promise((resolve) => addTimer(resolve, ms, [], false)),
    setNextRequest: () => warn('bru.setNextRequest is not supported: requests run in tree order'),
    runner: {
      skipRequest: () => pm.execution.skipRequest(),
      stopExecution: () => warn('bru.runner.stopExecution is not supported'),
      setNextRequest: () => warn('bru.runner.setNextRequest is not supported: requests run in tree order'),
    },
    runRequest: () => unsupported('bru.runRequest'),
    sendRequest: () => unsupported('bru.sendRequest'),
  };
  const req = {
    getUrl: () => url.toString(),
    setUrl: (u) => {
      request.url = u;
    },
    getMethod: () => String(request.method).toUpperCase(),
    setMethod: (m) => {
      request.method = String(m).toUpperCase();
    },
    getHeader: (n) => reqHeaders.get(n),
    getHeaders: () => reqHeaders.toObject(),
    setHeader: (n, v) => reqHeaders.upsert({ key: n, value: v }),
    setHeaders: (o) => {
      for (const [k, v] of Object.entries(o || {})) reqHeaders.upsert({ key: k, value: v });
    },
    deleteHeader: (n) => reqHeaders.remove(n),
    getBody: () => bodyValue(body.raw),
    setBody: (b) => {
      body.raw = typeof b === 'string' ? b : stringify(b);
    },
    getName: () => I.info.requestName,
    setTimeout: () => warn('req.setTimeout is not supported in scripts — use the request settings'),
    setMaxRedirects: () => warn('req.setMaxRedirects is not supported in scripts — use the request settings'),
    get url() {
      return url.toString();
    },
    get method() {
      return String(request.method).toUpperCase();
    },
    get headers() {
      return reqHeaders.toObject();
    },
    get body() {
      return bodyValue(body.raw);
    },
  };
  let res;
  if (response) {
    res = function (path) {
      return pathInfo(bodyValue(response.text()), path).value;
    };
    const lowerHeaders = () => Object.fromEntries(response.headers._items.map((h) => [h.key.toLowerCase(), h.value]));
    Object.assign(res, {
      status: response.code,
      statusText: response.status,
      responseTime: response.responseTime,
      getStatus: () => response.code,
      getStatusText: () => response.status,
      getHeader: (n) => response.headers.get(n),
      getHeaders: lowerHeaders,
      getBody: () => bodyValue(response.text()),
      getResponseTime: () => response.responseTime,
      getSize: () => response.size(),
      getUrl: () => url.toString(),
    });
    Object.defineProperty(res, 'headers', { get: lowerHeaders, enumerable: true });
    Object.defineProperty(res, 'body', { get: () => bodyValue(response.text()), enumerable: true });
  }

  // ---------------------------------------------------------------- globals

  const modules = {
    'crypto-js': CryptoJS,
    chai: { expect, AssertionError, assert: undefined },
    uuid: { v4: () => N.uuid(), validate: (s) => /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(String(s)) },
    lodash: _,
    atob: (s) => N.atob(String(s)),
    btoa: (s) => N.btoa(String(s)),
    buffer: { Buffer },
    tv4,
  };
  define('require', (name) => {
    if (hasOwn(modules, name)) return modules[name];
    throw new Error(`require('${name}') is not available in /dev/all scripts (available: ${Object.keys(modules).join(', ')})`);
  });
  define('pm', pm);
  define('postman', postman);
  define('expect', expect);
  define('test', test);
  define('bru', bru);
  define('req', req);
  if (res) define('res', res);
  define('CryptoJS', CryptoJS);
  define('_', _);
  define('tv4', tv4);
  define('btoa', modules.btoa);
  define('atob', modules.atob);
  define('crypto', crypto);
  define('Buffer', Buffer);
  if (typeof G.TextEncoder === 'undefined') define('TextEncoder', TextEncoder);
  if (typeof G.TextDecoder === 'undefined') define('TextDecoder', TextDecoder);
  define('setTimeout', (fn, ms, ...args) => addTimer(fn, ms, args, false));
  define('setInterval', (fn, ms, ...args) => addTimer(fn, ms, args, true));
  define('setImmediate', (fn, ...args) => addTimer(fn, 0, args, false));
  define('clearTimeout', (id) => timers.delete(id));
  define('clearInterval', (id) => timers.delete(id));
  define('clearImmediate', (id) => timers.delete(id));
  if (typeof G.queueMicrotask !== 'function') define('queueMicrotask', (fn) => Promise.resolve().then(fn));

  // Legacy (pre-`pm`) Postman sandbox globals.
  define('tests', {});
  define('environment', environment.toObject());
  define('globals', globals.toObject());
  define('data', { ...DATA });
  define('iteration', I.info.iteration);
  define('request', { url: url.toString(), method: request.method, headers: reqHeaders.toObject(), data: body.raw, name: I.info.requestName, id: I.info.requestId });
  if (response) {
    define('responseBody', response.text());
    define('responseCode', { code: response.code, name: response.status, detail: response.status });
    define('responseHeaders', response.headers.toObject());
    define('responseTime', response.responseTime);
    define('responseCookies', cookies.all());
  }

  // ---------------------------------------------------------------- results (read by Rust)

  function collect() {
    const out = [];
    for (const t of tests) {
      out.push({
        name: t.name,
        passed: t.done ? t.passed : false,
        skipped: t.skipped,
        message: t.done ? t.message : 'the test never finished: done() was not called or its promise never settled',
      });
    }
    const legacy = G.tests;
    if (legacy && typeof legacy === 'object') {
      for (const k of Object.keys(legacy)) out.push({ name: k, passed: !!legacy[k], skipped: false, message: legacy[k] ? null : 'test failed (tests["…"] was falsy)' });
    }
    return stringify({ request: PRE ? requestOut() : null, changes, tests: out, skipRequest });
  }

  Object.defineProperty(G, '__devall', { value: Object.freeze({ collect, nextTimer }), enumerable: false, writable: false, configurable: false });
})(globalThis.__devall_native, JSON.parse(globalThis.__devall_input), globalThis.__devall_body);
delete globalThis.__devall_native;
delete globalThis.__devall_input;
delete globalThis.__devall_body;
