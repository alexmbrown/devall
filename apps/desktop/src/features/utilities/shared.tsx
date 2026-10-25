import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { ClipboardPaste, Globe, Link2, Sparkles } from 'lucide-react';
import { copyLinkItems } from '@/components/links/LinkChip';
import { Menu } from '@/components/ui/Menu';
import { formatLink } from '@/lib/links/syntax';
import type { CodeLanguage } from '@/components/editor/CodeEditor';
import { Field, formatBytes } from '@/components/ui/misc';
import { errorMessage } from '@/lib/ipc';
import { selectOptions, utilsApi, type ArgInfo, type OpInfo, type UtilArgs } from '@/lib/api/utils';
import { historyApi, useLocalHistoryVersion } from '@/lib/api/history';
import { useWorkspace } from '@/store/workspace';

/** One-shot state handed to a tool when it is opened (deep link, magic → recipe, swap…). */
export interface Seed {
  args?: UtilArgs;
  nonce: number;
}

export interface UtilitiesCtx {
  ops: OpInfo[];
  /** Switch to another op, optionally replacing the shared input and overriding args. */
  open(opId: string, opts?: { input?: string; args?: UtilArgs }): void;
  pasteAndDetect(): void;
}

export const UtilitiesContext = createContext<UtilitiesCtx>({ ops: [], open: () => {}, pasteAndDetect: () => {} });
export const useUtilities = () => useContext(UtilitiesContext);

export interface ToolProps {
  op: OpInfo;
  /** Input shared across tools so a pasted value survives switching operations. */
  input: string;
  setInput(v: string): void;
  seed?: Seed;
}

/** Applies `seed.args` once per nonce. */
export function useSeed(seed: Seed | undefined, apply: (args: UtilArgs) => void) {
  const seen = useRef<number | undefined>(undefined);
  const applyRef = useRef(apply);
  applyRef.current = apply;
  useEffect(() => {
    if (!seed || seen.current === seed.nonce) return;
    seen.current = seed.nonce;
    if (seed.args) applyRef.current(seed.args);
  }, [seed]);
}

export interface RunState {
  output: string;
  error: string | null;
  running: boolean;
}

/**
 * Runs `op` whenever its inputs change (debounced, latest wins). With `manual`, only `trigger()`
 * runs it — used for network ops so typing a host doesn't fire DNS/TLS requests.
 */
export function useOpRunner(opId: string | undefined, input: string, args: UtilArgs, opts: { enabled?: boolean; manual?: boolean; delay?: number } = {}) {
  const { enabled = true, manual = false, delay = 140 } = opts;
  const [state, setState] = useState<RunState>({ output: '', error: null, running: false });
  const seq = useRef(0);
  const [tick, setTick] = useState(0);
  const argsKey = JSON.stringify(args);
  const latest = useRef({ input, argsKey });
  latest.current = { input, argsKey };
  // In manual mode edits don't re-run; only `trigger()` (tick) does, with the latest values.
  const inputDep = manual ? '' : input;
  const argsDep = manual ? '' : argsKey;

  useEffect(() => {
    if (!opId || !enabled || (manual && tick === 0)) {
      if (!enabled) setState({ output: '', error: null, running: false });
      return;
    }
    const n = ++seq.current;
    setState((s) => ({ ...s, running: true }));
    const t = setTimeout(
      () => {
        const { input: inp, argsKey: ak } = latest.current;
        utilsApi
          .run(opId, inp, JSON.parse(ak) as UtilArgs)
          .then((output) => n === seq.current && setState({ output, error: null, running: false }))
          .catch((e) => n === seq.current && setState({ output: '', error: errorMessage(e), running: false }));
      },
      manual ? 0 : delay,
    );
    return () => clearTimeout(t);
  }, [opId, inputDep, argsDep, enabled, tick, manual, delay]);

  const trigger = () => setTick((t) => t + 1);
  return { ...state, trigger };
}

/** How long input must stay unchanged before a run is recorded in utility history. */
const RECORD_AFTER_MS = 2000;

/**
 * Records successful runs of `op` in the device-wide utility history (Settings → History), once
 * input and arguments have been stable for 2 s. Returns a function that records right away (wire
 * it to "Copy output"). Secret arguments are masked by the backend.
 */
export function useRecordRun(op: OpInfo | undefined, input: string, args: UtilArgs, run: Pick<RunState, 'output' | 'error' | 'running'>, opts: { enabled?: boolean } = {}) {
  const root = useWorkspace((s) => s.project?.root ?? null);
  const latest = useRef({ op, input, args, run, root });
  latest.current = { op, input, args, run, root };
  const recorded = useRef('');
  const argsKey = JSON.stringify(args);
  const enabled = opts.enabled !== false;

  const record = useCallback(() => {
    const { op, input, args, run, root } = latest.current;
    if (!op || run.running || run.error || !run.output) return;
    const key = JSON.stringify([op.id, input, args]);
    if (key === recorded.current) return;
    recorded.current = key;
    historyApi
      .recordUtility({
        op: op.id,
        opName: op.name,
        input: op.noInput ? '' : input,
        args,
        secretArgs: op.args.filter((a) => a.kind === 'secret').map((a) => a.name),
        output: run.output,
        project: root,
      })
      .then((id) => id && useLocalHistoryVersion.getState().bumpUtilities())
      .catch(() => {
        recorded.current = '';
      });
  }, []);

  useEffect(() => {
    if (!enabled || !op || run.running || run.error || !run.output) return;
    const t = setTimeout(record, RECORD_AFTER_MS);
    return () => clearTimeout(t);
  }, [enabled, op?.id, input, argsKey, run.output, run.running, run.error, record]);

  return record;
}

export function ArgField({ arg, value, onChange, compact }: { arg: ArgInfo; value: string | undefined; onChange(v: string): void; compact?: boolean }) {
  const options = selectOptions(arg.kind);
  const v = value ?? arg.default;
  let control: ReactNode;
  if (options.length) {
    control = (
      <select className="select sm" value={v} onChange={(e) => onChange(e.target.value)}>
        {options.map((o) => (
          <option key={o}>{o}</option>
        ))}
      </select>
    );
  } else if (arg.kind === 'secret') {
    control = <textarea className="textarea mono" rows={1} style={{ minHeight: 28 }} value={value ?? ''} placeholder="never leaves this machine" onChange={(e) => onChange(e.target.value)} />;
  } else if (arg.kind === 'textarea') {
    control = <textarea className="textarea mono" rows={compact ? 2 : 4} value={value ?? ''} onChange={(e) => onChange(e.target.value)} spellCheck={false} />;
  } else {
    control = <input className="input sm mono" type={arg.kind === 'number' ? 'number' : 'text'} value={v} placeholder={arg.default} onChange={(e) => onChange(e.target.value)} spellCheck={false} />;
  }
  return (
    <Field label={arg.label} className={arg.kind === 'textarea' ? 'util-arg-wide' : 'util-arg'}>
      {control}
    </Field>
  );
}

export function NetworkBadge() {
  return (
    <span className="badge warning" title="Talks to the network — runs only when you click Run">
      <Globe /> network
    </span>
  );
}

/** Cycles through an op's examples on each click. */
export function ExampleButton({ op, onApply }: { op: OpInfo; onApply(input: string, args: UtilArgs): void }) {
  const idx = useRef(0);
  if (!op.examples.length) return null;
  return (
    <button
      className="btn btn-sm"
      title={op.examples.length > 1 ? `Load an example (${op.examples.length} available — click again for the next)` : 'Load an example'}
      onClick={() => {
        const ex = op.examples[idx.current % op.examples.length];
        idx.current += 1;
        onApply(ex.input, ex.args ?? {});
      }}
    >
      <Sparkles /> Example
    </button>
  );
}

const encoder = new TextEncoder();

/** "12 chars · 14 B · 3 lines" for a pane header. */
export function sizeLabel(s: string): string {
  if (s.length > 2_000_000) return `${formatBytes(s.length)}+`;
  let chars = 0;
  let lines = s ? 1 : 0;
  for (const c of s) {
    chars++;
    if (c === '\n') lines++;
  }
  return `${chars.toLocaleString()} chars · ${formatBytes(encoder.encode(s).length)}${lines > 1 ? ` · ${lines.toLocaleString()} lines` : ''}`;
}

/**
 * Header row above an input/output pane. It has a fixed height and compact controls, so panes
 * side by side line up (same header height → same editor top and height) whatever buttons each
 * header holds. Wrap the header and its body in `.util-pane`.
 */
export function PaneHead({ title, meta, lead, children, className }: { title?: ReactNode; meta?: ReactNode; lead?: ReactNode; children?: ReactNode; className?: string }) {
  return (
    <div className={className ? `util-pane-head ${className}` : 'util-pane-head'}>
      {title !== undefined && <span className="label">{title}</span>}
      {meta !== undefined && <span className="util-pane-meta">{meta}</span>}
      {lead !== undefined && <span className="util-pane-lead">{lead}</span>}
      <span className="spacer" />
      {children}
    </div>
  );
}

export function ToolHeader({ op, onExample, children }: { op: OpInfo; onExample?(input: string, args: UtilArgs): void; children?: ReactNode }) {
  const { pasteAndDetect } = useUtilities();
  return (
    <div className="page-header util-header">
      <h1>{op.name}</h1>
      {op.network && <NetworkBadge />}
      <span className="muted small truncate" title={op.description}>
        {op.description}
      </span>
      <span className="spacer" />
      {children}
      {onExample && <ExampleButton op={op} onApply={onExample} />}
      <Menu
        align="right"
        items={() => copyLinkItems(formatLink('utility', op.id), op.name)}
        trigger={(p) => (
          <button className="icon-btn" {...p} title="Copy a link to this utility (for notes and to-dos)" aria-label="Copy link">
            <Link2 />
          </button>
        )}
      />
      <button className="btn btn-sm" onClick={pasteAndDetect} title="Read the clipboard and pick the matching tool">
        <ClipboardPaste /> Paste &amp; detect
      </button>
    </div>
  );
}

const JSON_INPUT = new Set(['json.format', 'json.minify', 'json.toYaml', 'jq', 'jsonpath', 'jmespath', 'json.toTypes', 'diff.json', 'jwt.sign']);

export function inputLanguage(opId: string): CodeLanguage {
  if (JSON_INPUT.has(opId)) return 'json';
  if (opId === 'yaml.toJson') return 'yaml';
  return 'plaintext';
}

const TYPE_LANG: Record<string, string> = {
  TypeScript: 'typescript',
  'Rust (serde)': 'rust',
  Go: 'go',
  'Python (dataclasses)': 'python',
  'Python (pydantic)': 'python',
  'Java records': 'java',
  'C#': 'csharp',
  Kotlin: 'kotlin',
  Swift: 'swift',
  'JSON Schema': 'json',
};

/** Monaco language for an op's output (Monaco ships more grammars than `CodeLanguage` lists). */
export function outputLanguage(op: OpInfo, args: UtilArgs, output: string): CodeLanguage {
  if (op.id === 'json.toTypes') return (TYPE_LANG[args.language ?? 'TypeScript'] ?? 'plaintext') as CodeLanguage;
  if (op.id === 'json.toYaml') return 'yaml';
  if (op.output === 'json' || op.id === 'json.format' || op.id === 'yaml.toJson') return 'json';
  if (op.id === 'jq' || op.id === 'recipe.run') {
    const t = output.trimStart();
    return t.startsWith('{') || t.startsWith('[') ? 'json' : 'plaintext';
  }
  if (op.id === 'qr.generate') return 'xml';
  return 'plaintext';
}

/** Ops that can swap input/output with their inverse. */
export const INVERSE: Record<string, string> = {
  'base64.encode': 'base64.decode',
  'base64.decode': 'base64.encode',
  'url.encode': 'url.decode',
  'url.decode': 'url.encode',
  'html.encode': 'html.decode',
  'html.decode': 'html.encode',
  'hex.encode': 'hex.decode',
  'hex.decode': 'hex.encode',
  'json.toYaml': 'yaml.toJson',
  'yaml.toJson': 'json.toYaml',
  'json.escape': 'json.unescape',
  'json.unescape': 'json.escape',
  'gzip.compress': 'gzip.decompress',
  'gzip.decompress': 'gzip.compress',
};

export function microseconds(us: number): string {
  if (us < 1000) return `${us} µs`;
  if (us < 1_000_000) return `${(us / 1000).toFixed(us < 10_000 ? 2 : 1)} ms`;
  return `${(us / 1_000_000).toFixed(2)} s`;
}

/** Tiny localStorage helpers (UI convenience state only — never project data). */
export function loadLocal<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

export function saveLocal(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* storage full or unavailable */
  }
}
