import type { Filter, Level, LogEntry } from '@/lib/api/logs';

/** Bottom-to-top stacking order in the timeline (severe levels sit on the baseline). */
export const STACK_ORDER: Level[] = ['fatal', 'error', 'warn', 'info', 'debug', 'trace', 'unknown'];
/** Toolbar toggle order. */
export const TOGGLE_LEVELS: Level[] = ['fatal', 'error', 'warn', 'info', 'debug', 'trace', 'unknown'];

export const LEVEL_LABEL: Record<Level, string> = {
  trace: 'Trace',
  debug: 'Debug',
  info: 'Info',
  warn: 'Warn',
  error: 'Error',
  fatal: 'Fatal',
  unknown: 'Other',
};

/** CSS custom property holding each level's colour (defined in logs.css from tokens). */
export const levelVar = (l: Level) => `var(--lvl-${l})`;

export const EMPTY_FILTER: Filter = { query: '', levels: [], from: null, to: null };

export function isTrivial(f: Filter): boolean {
  return !f.query.trim() && f.levels.length === 0 && f.from == null && f.to == null;
}

export function filterKey(f: Filter): string {
  return JSON.stringify([f.query.trim(), [...f.levels].sort(), f.from ?? null, f.to ?? null]);
}

const pad = (n: number, w = 2) => String(n).padStart(w, '0');

/** `HH:MM:SS.mmm`, with `MM-DD` in front when `withDate`. Local time. */
export function fmtTs(ts: number | null | undefined, withDate = false): string {
  if (ts == null) return '';
  const d = new Date(ts);
  const t = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
  return withDate ? `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${t}` : t;
}

export function fmtFull(ts: number | null | undefined): string {
  if (ts == null) return '—';
  const d = new Date(ts);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${fmtTs(ts)}`;
}

export function fmtCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1e6) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  if (n < 1e9) return `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0)}M`;
  return `${(n / 1e9).toFixed(1)}B`;
}

export function fmtDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(s < 10 ? 2 : 1)} s`;
  const m = s / 60;
  if (m < 60) return `${m.toFixed(1)} min`;
  const h = m / 60;
  if (h < 48) return `${h.toFixed(1)} h`;
  return `${(h / 24).toFixed(1)} d`;
}

export function fmtSpan(from: number, to: number): string {
  const sameDay = new Date(from).toDateString() === new Date(to - 1).toDateString();
  return `${fmtTs(from, true)} – ${fmtTs(to, !sameDay)}`;
}

export interface Preset {
  id: string;
  label: string;
  ms: number | null;
}

export const PRESETS: Preset[] = [
  { id: 'all', label: 'All time', ms: null },
  { id: '5m', label: 'Last 5 min', ms: 5 * 60_000 },
  { id: '15m', label: 'Last 15 min', ms: 15 * 60_000 },
  { id: '1h', label: 'Last hour', ms: 3_600_000 },
  { id: '6h', label: 'Last 6 hours', ms: 6 * 3_600_000 },
  { id: '24h', label: 'Last 24 hours', ms: 86_400_000 },
  { id: '7d', label: 'Last 7 days', ms: 7 * 86_400_000 },
];

/** Keys that the viewer already shows as columns (not repeated as chips). */
const MAPPED = new Set([
  '@timestamp', 'timestamp', 'time', 'ts', '@t', 'datetime', 'date', 't', 'eventtime', 'event_time', 'logged_at', '__realtime_timestamp',
  'level', 'severity', 'lvl', '@l', 'loglevel', 'log_level', 'levelname', 'level_name', 'severitytext', 'severity_text', '@level', 'priority',
  'msg', 'message', '@m', '@message', '@mt', 'log', 'text', 'event', 'body', 'short_message', 'full_message',
  'data', 'id', 'source', 'kind', 'correlationid', 'requestpath',
]);

export function chipFields(e: LogEntry, max = 4): [string, unknown][] {
  const out: [string, unknown][] = [];
  for (const [k, v] of Object.entries(e.fields)) {
    if (MAPPED.has(k.toLowerCase()) || v == null || typeof v === 'object') continue;
    const s = String(v);
    if (s.length > 60 || (typeof v === 'string' && e.message.includes(`${k}=${s}`) && e.format !== 'logfmt')) continue;
    out.push([k, v]);
    if (out.length >= max) break;
  }
  return out;
}

export function valueText(v: unknown): string {
  if (v == null) return 'null';
  if (typeof v === 'string') return v;
  return JSON.stringify(v);
}

/** First line of a message plus how many more lines it has. */
export function firstLine(s: string): [string, number] {
  const i = s.indexOf('\n');
  if (i < 0) return [s, 0];
  let n = 0;
  for (let k = i; k >= 0; k = s.indexOf('\n', k + 1)) n++;
  return [s.slice(0, i), n];
}

/** Adds a clause to a query (AND), avoiding duplicates. */
export function addClause(query: string, clause: string): string {
  const q = query.trim();
  if (!q) return clause;
  if (q.split(/\s+/).includes(clause)) return q;
  const needsParens = /\bOR\b|\|\|/.test(q);
  return needsParens ? `(${q}) ${clause}` : `${q} ${clause}`;
}

export function pretty(e: LogEntry): string {
  const t = e.raw.trim();
  if (t.startsWith('{') || t.startsWith('[')) {
    try {
      return JSON.stringify(JSON.parse(t), null, 2);
    } catch {
      /* not JSON */
    }
  }
  const lines = [e.message];
  if (Object.keys(e.fields).length) lines.push('', JSON.stringify(e.fields, null, 2));
  return lines.join('\n');
}
