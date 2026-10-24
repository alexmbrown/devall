import { invoke } from '@tauri-apps/api/core';
import { isPluginOp, pluginsApi, usePluginCatalog } from './plugins';

/**
 * Typed wrappers over the `util_list` / `util_run` Tauri commands (devall-utils registry) plus TS
 * mirrors of the op metadata and of the JSON results the specialized Utilities views read.
 *
 * NOTE for the integrator: `OpInfo` here supersedes the copy in `lib/types.ts` (it adds `network`
 * and `examples`); the old one can re-export this.
 */

export interface ArgInfo {
  name: string;
  label: string;
  /** `text`, `secret`, `number`, `textarea` or `select:a|b|c`. */
  kind: string;
  default: string;
}

export interface OpExample {
  input: string;
  /** Argument overrides (strings); `{}` when the defaults are fine. */
  args: Record<string, string>;
}

export interface OpInfo {
  id: string;
  name: string;
  category: string;
  description: string;
  output: 'text' | 'json';
  /** Generators that ignore their input. */
  noInput: boolean;
  args: ArgInfo[];
  /** Talks to the network (DNS / TLS): badge it and only run on request. */
  network: boolean;
  examples: OpExample[];
}

export type UtilArgs = Record<string, string>;

/** Built-in ops run in Rust; `<plugin>:<op>` ids run in the plugin's WebAssembly sandbox. */
async function runOp(op: string, input: string, args: UtilArgs): Promise<string> {
  if (isPluginOp(op)) return (await pluginsApi.runUtility(usePluginCatalog.getState().root, op, input, args)).output;
  return invoke<string>('util_run', { op, input, args });
}

export const utilsApi = {
  list: () => invoke<OpInfo[]>('util_list'),
  run: (op: string, input: string, args: UtilArgs = {}) => runOp(op, input, args),
  /** Runs an op whose output is JSON and parses it. */
  runJson: async <T>(op: string, input: string, args: UtilArgs = {}) => JSON.parse(await runOp(op, input, args)) as T,
};

/** Options of a `select:a|b` arg kind (empty for other kinds). */
export function selectOptions(kind: string): string[] {
  return kind.startsWith('select:') ? kind.slice(7).split('|') : [];
}

export function defaultArgs(op: OpInfo): UtilArgs {
  return Object.fromEntries(op.args.map((a) => [a.name, a.default]));
}

// ── Result shapes ───────────────────────────────────────────────────────────────────────────────

/** UTF-16 offsets (`start`/`end`) index straight into JS strings. */
export interface RegexSpan {
  start: number;
  end: number;
  byteStart: number;
  byteEnd: number;
  text: string;
}

export interface RegexGroup extends Partial<RegexSpan> {
  index: number;
  name: string | null;
  matched?: false;
}

export interface RegexMatch extends RegexSpan {
  index: number;
  groups: RegexGroup[];
}

export interface RegexTestResult {
  flavor: 'rust' | 'pcre-like';
  pattern: string;
  flags: string;
  matchCount: number;
  truncated: boolean;
  groupCount: number;
  groupNames: { index: number; name: string | null }[];
  compileMicros: number;
  matchMicros: number;
  matches: RegexMatch[];
  warnings: string[];
  error: string | null;
}

export interface RegexNode {
  kind: string;
  text: string;
  description: string;
  /** UTF-16 offsets into the pattern (absent for the backtracking-parser fallback). */
  start?: number;
  end?: number;
  children?: RegexNode[];
}

/** `regex.cases` output: each saved case and whether the pattern behaves as expected. */
export interface RegexCaseResult {
  text: string;
  shouldMatch: boolean;
  matched: boolean;
  match: string | null;
  passed: boolean;
  error: string | null;
}

export interface RegexCasesResult {
  passed: number;
  failed: number;
  cases: RegexCaseResult[];
}

export interface RegexExplainResult {
  flavor: string;
  source: 'ast' | 'backtracking-parser';
  nodes: RegexNode[];
  warnings: string[];
}

export interface TextDiffChange {
  tag: 'equal' | 'delete' | 'insert';
  oldLine: number | null;
  newLine: number | null;
  value: string;
  segments?: { emphasized: boolean; value: string }[];
}

export interface TextDiffResult {
  mode: 'line' | 'word' | 'char';
  identical: boolean;
  similarity: number;
  stats: { inserted: number; deleted: number; unit: string };
  unified: string;
  hunks?: { header: string; oldStart: number; oldLines: number; newStart: number; newLines: number; changes: TextDiffChange[] }[];
  segments?: { tag: 'equal' | 'delete' | 'insert'; value: string }[];
}

export interface JsonDifference {
  op: 'added' | 'removed' | 'changed';
  path: string;
  pointer: string;
  old?: unknown;
  new?: unknown;
}

export interface JsonDiffResult {
  equal: boolean;
  ignoreArrayOrder: boolean;
  summary: { added: number; removed: number; changed: number };
  differences: JsonDifference[];
  patch: { op: 'add' | 'remove' | 'replace'; path: string; value?: unknown }[];
  note?: string;
}

export interface CompareCharInfo {
  char: string;
  codePoint: string;
  name: string | null;
  script: string;
}

export interface CompareSide {
  chars: number;
  graphemes: number;
  utf8Bytes: number;
  utf16Units: number;
  lines: number;
  lineEndings: 'none' | 'LF' | 'CRLF' | 'CR' | 'mixed';
  bom: boolean;
  trailingNewline: boolean;
  leadingWhitespace: boolean;
  trailingWhitespace: boolean;
  nonAscii: number;
  isNfc: boolean;
  isNfkc: boolean;
  whitespace: { spaces: number; tabs: number; cr: number; lf: number; other: number };
  sha256: string;
}

export interface CompareFinding {
  side: 'a' | 'b';
  index: number;
  char: string;
  codePoint: string;
  name: string | null;
  kind: 'bom' | 'bidi' | 'invisible' | 'control' | 'whitespace' | 'confusable';
  note: string;
}

/** `text.compare` (String compare). */
export interface CompareResult {
  equal: {
    exact: boolean;
    ignoreCase: boolean;
    trimmed: boolean;
    lineEndings: boolean;
    whitespace: boolean;
    nfc: boolean;
    nfkc: boolean;
    invisibleRemoved: boolean;
    lookalikes: boolean;
    loose: boolean;
  };
  hints: string[];
  a: CompareSide;
  b: CompareSide;
  firstDifference: {
    index: number;
    line: number;
    column: number;
    utf16Offset: number;
    byteOffsetA: number;
    byteOffsetB: number;
    a: CompareCharInfo | null;
    b: CompareCharInfo | null;
    before: string;
    afterA: string;
    afterB: string;
  } | null;
  segments: { tag: 'equal' | 'delete' | 'insert'; value: string }[] | null;
  diffSkipped: boolean;
  findings: CompareFinding[];
  findingCounts: { a: number; b: number };
}

export interface CronResult {
  expression: string;
  normalized: string;
  description: string;
  fields: Record<string, string>;
  timezone: string;
  nextRuns: { time: string; display: string; utc: string; weekday: string; relative: string }[];
  warnings: string[];
}

export interface RecipeStep {
  op: string;
  args?: UtilArgs;
  disabled?: boolean;
}

export interface MagicCandidate {
  recipe: RecipeStep[];
  label: string;
  score: number;
  reasons: string[];
  preview: string;
  truncated: boolean;
}

export interface MagicResult {
  input: { length: number; printableRatio: number };
  explored: number;
  candidates: MagicCandidate[];
}

export interface ColorResult {
  hex: string;
  hexAlpha: string | null;
  rgb: string;
  hsl: string;
  hsv: string;
  oklab: string;
  oklch: string;
  nearestNamed: { name: string; exact: boolean } | null;
  relativeLuminance: number;
  bestTextColor: 'black' | 'white';
  outOfSrgbGamut: boolean;
  contrast?: { against: string; ratio: number; aaNormalText: boolean; aaLargeText: boolean; aaaNormalText: boolean; aaaLargeText: boolean; uiComponents: boolean };
}
