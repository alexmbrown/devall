import { Fragment, type ReactNode } from 'react';
import { Box, FileText, Link2, ScrollText, Server } from 'lucide-react';
import type { SourceRef } from '@/lib/api/ai';
import { useNav } from '@/store/nav';
import { openInLogViewer } from '@/features/logs';
import { useInfraUi } from '@/features/infra/store';
import { sourceTarget } from './sourceTarget';

/** Opens a cited source: the stub in Mocks, the event in Logs, the pod's namespace in Docker & K8s. */
export function openSource(ref: SourceRef) {
  const t = sourceTarget(ref);
  const go = useNav.getState().go;
  switch (t.type) {
    case 'stub':
      go('mocks', `stubs/file:${t.file}`);
      break;
    case 'event':
      openInLogViewer({ type: 'events' }, { query: `id="${t.id}"` });
      break;
    case 'pod':
      useInfraUi.getState().set({ tab: 'kube', kubeContext: t.context || undefined, kubeNamespace: t.namespace });
      go('infra');
      break;
    case 'note':
      go('notes', t.path);
      break;
    default:
      break;
  }
}

function iconFor(kind: string) {
  if (kind.startsWith('mock')) return <Server size={12} />;
  if (kind === 'event') return <ScrollText size={12} />;
  if (kind.startsWith('pod')) return <Box size={12} />;
  if (kind === 'note') return <FileText size={12} />;
  return <Link2 size={12} />;
}

/** Text with `[S2]` markers turned into buttons that open the source. */
export function linkedText(text: string, sources: SourceRef[]): ReactNode {
  const parts = text.split(/(\[S\d+(?:\s*[,;]\s*S\d+)*\])/g);
  return parts.map((p, i) => {
    const m = /^\[(S\d+(?:\s*[,;]\s*S\d+)*)\]$/.exec(p);
    if (!m) return <Fragment key={i}>{p}</Fragment>;
    const ids = m[1].split(/[,;]/).map((x) => x.trim());
    return (
      <Fragment key={i}>
        {ids.map((id) => {
          const ref = sources.find((s) => s.id === id);
          return ref ? (
            <button key={id} className="ai-cite-mark" onClick={() => openSource(ref)} title={`${ref.title} — ${ref.source}`}>
              {id.slice(1)}
            </button>
          ) : (
            <span key={id}>[{id}]</span>
          );
        })}
      </Fragment>
    );
  });
}

/** The numbered sources of an answer; cited ones first, each clickable when it can be opened. */
export function SourceList({ sources, cited }: { sources: SourceRef[]; cited: SourceRef[] }) {
  if (sources.length === 0) return null;
  const isCited = (s: SourceRef) => cited.some((c) => c.id === s.id);
  return (
    <div className="ai-sources">
      <div className="xs subtle">Sources</div>
      {sources.map((s) => {
        const openable = sourceTarget(s).type !== 'none' && sourceTarget(s).type !== 'request';
        return (
          <button key={s.id} className="ai-source-row" onClick={() => openable && openSource(s)} disabled={!openable} title={s.source}>
            <span className={isCited(s) ? 'badge info' : 'badge'}>{s.id}</span>
            {iconFor(s.kind)}
            <span className="truncate small">{s.title}</span>
          </button>
        );
      })}
    </div>
  );
}
