/** Splits a string into plain text and `{{reference}}` segments (same rules as the Rust resolver). */
export type Segment = { kind: 'text'; text: string } | { kind: 'var'; text: string; name: string };

export function segments(input: string): Segment[] {
  const out: Segment[] = [];
  let rest = input;
  while (rest.length) {
    const start = rest.indexOf('{{');
    if (start < 0) break;
    const end = rest.indexOf('}}', start + 2);
    if (end < 0) break;
    if (start > 0) out.push({ kind: 'text', text: rest.slice(0, start) });
    const text = rest.slice(start, end + 2);
    out.push({ kind: 'var', text, name: text.slice(2, -2).trim() });
    rest = rest.slice(end + 2);
  }
  if (rest) out.push({ kind: 'text', text: rest });
  return out;
}

/** The `{{partial` being typed at `caret`, if any (for autocomplete). */
export function openReference(value: string, caret: number): { start: number; query: string } | null {
  const before = value.slice(0, caret);
  const start = before.lastIndexOf('{{');
  if (start < 0 || before.indexOf('}}', start) >= 0) return null;
  const query = before.slice(start + 2);
  if (/\s{2,}|\n/.test(query)) return null;
  return { start, query: query.trim() };
}
