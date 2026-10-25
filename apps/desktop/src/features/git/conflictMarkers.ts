/**
 * Conflict markers in a file's text (`<<<<<<< ours`, `||||||| base` with diff3/zdiff3,
 * `=======`, `>>>>>>> theirs`) — found, navigated and resolved one chunk at a time in the merge
 * editor's result pane. Pure, so it's testable. CRLF files keep their line endings.
 */

export interface ConflictChunk {
  /** Line index (0-based) of the `<<<<<<<` marker. */
  start: number;
  /** Line index of the `>>>>>>>` marker. */
  end: number;
  ours: string[];
  /** `null` without a base section (merge style). */
  base: string[] | null;
  theirs: string[];
  oursLabel: string;
  theirsLabel: string;
}

export type ChunkChoice = 'ours' | 'theirs' | 'base' | 'oursThenTheirs' | 'theirsThenOurs';

const marker = (line: string, ch: string) => {
  const m = new RegExp(`^\\${ch}{7,}(?: (.*))?$`).exec(line);
  return m ? (m[1] ?? '') : null;
};
const isSeparator = (line: string) => /^={7,}$/.test(line);

function lines(text: string): { lines: string[]; eol: string } {
  const eol = text.includes('\r\n') ? '\r\n' : '\n';
  return { lines: text.split('\n').map((l) => (l.endsWith('\r') ? l.slice(0, -1) : l)), eol };
}

/** Every complete conflict chunk, top to bottom. */
export function parseConflicts(text: string): ConflictChunk[] {
  const { lines: ls } = lines(text);
  const out: ConflictChunk[] = [];
  let i = 0;
  while (i < ls.length) {
    const oursLabel = marker(ls[i], '<');
    if (oursLabel === null) {
      i++;
      continue;
    }
    const start = i;
    const ours: string[] = [];
    let base: string[] | null = null;
    const theirs: string[] = [];
    let section: 'ours' | 'base' | 'theirs' = 'ours';
    let end = -1;
    let theirsLabel = '';
    for (i = start + 1; i < ls.length; i++) {
      const l = ls[i];
      if (section !== 'theirs' && marker(l, '|') !== null) {
        section = 'base';
        base = [];
        continue;
      }
      if (section !== 'theirs' && isSeparator(l)) {
        section = 'theirs';
        continue;
      }
      const close = section === 'theirs' ? marker(l, '>') : null;
      if (close !== null) {
        end = i;
        theirsLabel = close;
        break;
      }
      if (marker(l, '<') !== null) break; // a new chunk before this one closed: malformed
      (section === 'ours' ? ours : section === 'base' ? base! : theirs).push(l);
    }
    if (end < 0) {
      i = start + 1;
      continue;
    }
    out.push({ start, end, ours, base, theirs, oursLabel, theirsLabel });
    i = end + 1;
  }
  return out;
}

export function hasMarkers(text: string): boolean {
  return parseConflicts(text).length > 0;
}

/** The lines a choice puts in place of the chunk. */
export function chunkLines(c: ConflictChunk, choice: ChunkChoice): string[] {
  switch (choice) {
    case 'ours':
      return c.ours;
    case 'theirs':
      return c.theirs;
    case 'base':
      return c.base ?? [];
    case 'oursThenTheirs':
      return [...c.ours, ...c.theirs];
    case 'theirsThenOurs':
      return [...c.theirs, ...c.ours];
  }
}

/** The text with chunk `index` replaced by `choice` (other chunks untouched). */
export function resolveChunk(text: string, index: number, choice: ChunkChoice): string {
  const chunks = parseConflicts(text);
  const c = chunks[index];
  if (!c) return text;
  const { lines: ls, eol } = lines(text);
  const next = [...ls.slice(0, c.start), ...chunkLines(c, choice), ...ls.slice(c.end + 1)];
  return next.join(eol);
}

/** Every chunk resolved the same way. */
export function resolveAll(text: string, choice: ChunkChoice): string {
  let t = text;
  while (parseConflicts(t).length) {
    const before = t;
    t = resolveChunk(t, 0, choice);
    if (t === before) break;
  }
  return t;
}

/** The chunk after (`delta` 1) or before (-1) the cursor line (1-based), wrapping around. */
export function nearestChunk(chunks: ConflictChunk[], cursorLine: number, delta: 1 | -1): number {
  if (!chunks.length) return -1;
  const at = cursorLine - 1;
  if (delta > 0) {
    const i = chunks.findIndex((c) => c.start > at);
    return i >= 0 ? i : 0;
  }
  for (let i = chunks.length - 1; i >= 0; i--) if (chunks[i].end < at) return i;
  return chunks.length - 1;
}

/** The chunk containing the cursor line (1-based), else -1. */
export function chunkAt(chunks: ConflictChunk[], cursorLine: number): number {
  const at = cursorLine - 1;
  return chunks.findIndex((c) => c.start <= at && at <= c.end);
}
