import type { ReactNode } from 'react';
import { BookOpenCheck, Folder, NotebookText } from 'lucide-react';
import { MethodBadge } from '@/components/ui/misc';
import { SectionIcon } from '@/components/layout/sectionIcons';
import type { ProtoKind } from '@/lib/api/proto';
import { sectionMeta } from '@/lib/sections';
import type { View } from '@/store/workbench';
import { docForParam, findTreeNode, isDirty, parseDocParam, useWorkspace } from '@/store/workspace';

export interface ViewInfo {
  title: string;
  /** Section name shown dimmed before the title ("To-dos: Today"), so a tab named after a list,
   * filter, environment or project still says which module it is. */
  prefix?: string;
  icon: ReactNode;
  tooltip: string;
  /** Unsaved changes (dirty dot on the tab). */
  dirty?: boolean;
  /** Work in flight (spinner on the tab), e.g. a request being sent. */
  busy?: boolean;
}

const PROTO_BADGE: Record<ProtoKind, [string, string]> = { ws: ['WS', 'WebSocket'], grpc: ['GRPC', 'gRPC'], mqtt: ['MQTT', 'MQTT'], socketio: ['SIO', 'Socket.IO'] };

/** Protocol badge of a WebSocket/gRPC/MQTT/Socket.IO tab (kept here so the tab strip doesn't load the editors). */
function ProtoBadge({ kind }: { kind: ProtoKind }) {
  return (
    <span className={`badge proto-kind ${kind}`} title={PROTO_BADGE[kind][1]}>
      {PROTO_BADGE[kind][0]}
    </span>
  );
}

const stem = (path: string) => path.split('/').pop()?.replace(/\.(md|ipynb)$/, '') ?? path;

/** Title, icon and state of a tab. Document views describe their document (method + request name, note name…). */
export function useViewInfo(view: View): ViewInfo {
  const meta = sectionMeta(view.section);
  const param = view.param;
  const isHttpDoc = view.section === 'http' && !!param;
  // Narrow subscriptions: only http document tabs read the workspace.
  const doc = useWorkspace((s) => (isHttpDoc ? docForParam(s.tabs, param) : undefined));
  const node = useWorkspace((s) => (isHttpDoc ? findTreeNode(s.tree, parseDocParam(param!).path) : undefined));

  if (isHttpDoc) {
    const { kind, path, protocol } = parseDocParam(param!);
    const tooltip = `http/${param}`;
    if (kind === 'folder') {
      const name = (doc?.kind === 'folder' ? doc.draft.name : undefined) ?? (path ? (path.split('/').pop() ?? path) : 'Project root');
      return { title: name, icon: <Folder size={13} />, tooltip: `${tooltip} — folder settings`, dirty: !!doc && isDirty(doc) };
    }
    if (kind === 'proto') {
      const name = (doc?.kind === 'proto' ? doc.name : undefined) ?? node?.name ?? stem(path);
      return { title: name, icon: <ProtoBadge kind={protocol!} />, tooltip };
    }
    const req = doc?.kind === 'request' ? doc : undefined;
    const method = req?.draft.method ?? (node?.kind === 'request' ? node.method : undefined);
    const name = req?.draft.name ?? node?.name ?? stem(path).replace(/\.req\.yaml$/, '');
    return { title: name, icon: <MethodBadge method={method} short />, tooltip, dirty: !!req && isDirty(req), busy: !!req?.sendId };
  }
  if (view.section === 'notes' && param) {
    return { title: stem(param), prefix: meta.short ?? meta.label, icon: param.endsWith('.ipynb') ? <BookOpenCheck size={13} /> : <NotebookText size={13} />, tooltip: `notes/${param}` };
  }
  const name = meta.title ?? meta.label;
  // Pages label their view with what it shows (useViewLabel), e.g. the selected utility.
  return {
    title: view.label ?? meta.label,
    prefix: view.label ? (meta.short ?? meta.label) : undefined,
    icon: <SectionIcon section={view.section} size={13} />,
    tooltip: view.label ? `${name} — ${view.label}` : name,
  };
}
