import type { SourceRef } from '@/lib/api/ai';

/**
 * Turns `[S1]` / `[S2, S3]` citation markers into small Markdown links (`[1](cite/S1)`) that the
 * Ask tab's link handler maps back to sources. Unknown ids are left untouched.
 */
export function linkCitations(text: string, sources: SourceRef[]): string {
  const known = new Set(sources.map((s) => s.id));
  return text.replace(/\[((?:S\d+\s*[,;]?\s*)+)\]/g, (whole, inner: string) => {
    const ids = inner
      .split(/[,;]/)
      .map((x) => x.trim())
      .filter((x) => known.has(x));
    if (!ids.length) return whole;
    return ids.map((id) => `[${id.slice(1)}](cite/${id})`).join('');
  });
}
