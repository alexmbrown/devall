import type { SourceRef } from '@/lib/api/ai';

/** Where a cited source opens. */
export type SourceTarget =
  | { type: 'stub'; file: string; id?: string }
  | { type: 'event'; id: string }
  | { type: 'pod'; context: string; namespace: string; name: string }
  | { type: 'note'; path: string }
  | { type: 'request' }
  | { type: 'none' };

/** Parses a source reference (`mocks/mappings/x.json#id`, `event:<id>`, `pod:<ctx>/<ns>/<name>`, `notes/a.md`). */
export function sourceTarget(ref: Pick<SourceRef, 'kind' | 'source'>): SourceTarget {
  const s = ref.source;
  if (s.startsWith('event:')) return { type: 'event', id: s.slice(6) };
  if (s.startsWith('pod:')) {
    const parts = s.slice(4).split('/');
    if (parts.length >= 3) return { type: 'pod', context: parts.slice(0, -2).join('/'), namespace: parts[parts.length - 2], name: parts[parts.length - 1] };
  }
  if (ref.kind === 'mockStub' || ref.kind === 'mockNearMiss') {
    const [file, id] = s.split('#');
    if (file) return { type: 'stub', file: file.replace(/^mocks\//, ''), id: id || undefined };
  }
  if (s.startsWith('notes/')) return { type: 'note', path: s.slice(6) };
  if (ref.kind === 'exchange') return { type: 'request' };
  return { type: 'none' };
}
