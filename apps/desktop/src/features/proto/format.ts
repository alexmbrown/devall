/** Pure helpers for the message log (unit-tested in format.test.ts). */

/** Pretty JSON when the text parses, otherwise the text itself. */
export function prettyBody(body: string): { text: string; json: boolean } {
  const t = body.trim();
  if (t.startsWith('{') || t.startsWith('[') || t.startsWith('"')) {
    try {
      return { text: JSON.stringify(JSON.parse(t), null, 2), json: true };
    } catch {
      /* not JSON */
    }
  }
  return { text: body, json: false };
}

/** Classic hex dump (offset, 16 bytes, ASCII) of base64 data; the first 512 bytes. */
export function hexDump(base64: string): string {
  let bytes: Uint8Array;
  try {
    bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  } catch {
    return '';
  }
  const lines: string[] = [];
  const n = Math.min(bytes.length, 512);
  for (let i = 0; i < n; i += 16) {
    const chunk = Array.from(bytes.slice(i, Math.min(i + 16, n)));
    const hex = chunk.map((b) => b.toString(16).padStart(2, '0')).join(' ');
    const ascii = chunk.map((b) => (b >= 32 && b < 127 ? String.fromCharCode(b) : '.')).join('');
    lines.push(`${i.toString(16).padStart(6, '0')}  ${hex.padEnd(47)}  ${ascii}`);
  }
  if (bytes.length > n) lines.push(`… ${bytes.length - n} more bytes`);
  return lines.join('\n');
}
