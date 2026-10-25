import type { ReactNode } from 'react';
import { Activity, Binary, CheckCircle2, Circle, Database, Dices, FileCode2, Folder, GitBranch, Layers, Link2, NotebookText, Send, Server, Webhook } from 'lucide-react';
import type { LinkKind } from '@/lib/links/syntax';

/** Default icon of each kind (providers usually give richer ones: method badge, environment colour…). */
export function kindIcon(kind: LinkKind | null): ReactNode {
  switch (kind) {
    case 'request':
      return <Send />;
    case 'folder':
      return <Folder />;
    case 'note':
      return <NotebookText />;
    case 'todo':
      return <Circle />;
    case 'env':
    case 'variables':
      return <Layers />;
    case 'stub':
      return <Server />;
    case 'load':
      return <Activity />;
    case 'fakedata':
      return <Dices />;
    case 'query':
      return <FileCode2 />;
    case 'connection':
      return <Database />;
    case 'webhook':
      return <Webhook />;
    case 'utility':
      return <Binary />;
    case 'repo':
      return <GitBranch />;
    default:
      return <Link2 />;
  }
}

/** A to-do's check circle (static), coloured by priority. */
export function TodoIcon({ done, priority }: { done?: boolean; priority?: number }) {
  return <span className={`dl-todo p${priority ?? 4}`}>{done ? <CheckCircle2 /> : <Circle />}</span>;
}
