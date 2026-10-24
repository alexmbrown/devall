import type { DriftKind, Severity } from '@/lib/api/openapi';

const KIND_LABELS: Record<DriftKind, string> = {
  undocumentedPath: 'Undocumented path',
  undocumentedMethod: 'Undocumented method',
  undocumentedStatus: 'Undocumented status',
  invalidResponseBody: 'Response body',
  invalidRequestBody: 'Request body',
  undocumentedContentType: 'Content type',
  unverifiable: 'Not checked',
};

export function kindLabel(k: DriftKind): string {
  return KIND_LABELS[k] ?? k;
}

/** Severity + free-text filter (case-insensitive, every word must appear). */
export function filterFindings<T extends { severity: Severity }>(items: T[], severity: 'all' | Severity, query: string, text: (t: T) => string): T[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return items.filter((f) => {
    if (severity !== 'all' && f.severity !== severity) return false;
    if (!words.length) return true;
    const hay = text(f).toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

/** Project-changed paths that could be specs (YAML/JSON outside the module folders). */
export function specCandidates(paths: string[]): string[] {
  return paths.filter((p) => /\.(ya?ml|json)$/i.test(p) && !/^(http|mocks|load|environments|\.devall)\//.test(p) && !p.endsWith('.req.yaml') && !p.endsWith('.env.yaml'));
}

/** `C:\\proj\\specs\\a.yaml` under root `C:\\proj` → `specs/a.yaml`; other paths unchanged. */
export function toProjectPath(root: string, path: string): string {
  const norm = (s: string) => s.replace(/\\/g, '/').replace(/\/+$/, '');
  const r = norm(root);
  const p = norm(path);
  const same = (a: string, b: string) => (/^[a-z]:/i.test(a) ? a.toLowerCase() === b.toLowerCase() : a === b);
  if (p.length > r.length && same(p.slice(0, r.length), r) && p[r.length] === '/') return p.slice(r.length + 1);
  return path;
}
