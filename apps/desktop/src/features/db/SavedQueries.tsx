import { useCallback, useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { AlertTriangle, ChevronDown, ChevronRight, FileCode2, FileJson, FolderOpen, History, Plus, Terminal, Trash2 } from 'lucide-react';
import { openLocalHistory } from '@/features/history/LocalHistoryDialog';
import { useContextMenu } from '@/components/ui/Menu';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { dbApi, type ConnectionEntry, type QueryKind, type SavedQueryEntry } from '@/lib/api/db';

const ICON: Record<QueryKind, React.ReactNode> = {
  sql: <FileCode2 size={13} className="text-accent" />,
  redis: <Terminal size={13} className="text-danger" />,
  mongo: <FileJson size={13} className="text-success" />,
};

interface Props {
  root: string;
  /** Selected connection (filters the list and is used for new queries). */
  selected?: ConnectionEntry;
  onOpen(entry: SavedQueryEntry): void;
}

/** `queries/**` bound to the project: `*.sql`, `*.redis`, `*.mongo.json`. */
export function SavedQueries({ root, selected, onOpen }: Props) {
  const [items, setItems] = useState<SavedQueryEntry[]>([]);
  const [all, setAll] = useState(false);
  const [open, setOpen] = useState(true);
  const ctx = useContextMenu();

  const load = useCallback(async () => {
    try {
      setItems(await dbApi.queries(root));
    } catch {
      setItems([]);
    }
  }, [root]);

  useEffect(() => {
    void load();
    const on = () => void load();
    window.addEventListener('focus', on);
    window.addEventListener('devall:db-queries-changed', on);
    return () => {
      window.removeEventListener('focus', on);
      window.removeEventListener('devall:db-queries-changed', on);
    };
  }, [load]);

  const kind: QueryKind | undefined = selected?.kind === 'redis' ? 'redis' : selected?.kind === 'mongodb' ? 'mongo' : selected?.kind ? 'sql' : undefined;
  const shown = useMemo(() => (all || !selected ? items : items.filter((q) => q.connection === selected.slug || (!q.connection && q.kind === kind))), [items, all, selected, kind]);

  const create = async () => {
    if (!selected || !kind) return;
    const name = await promptDialog({ title: 'New saved query', label: 'Name', placeholder: 'Slow orders', validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!name) return;
    try {
      const text = kind === 'sql' ? 'SELECT 1;\n' : kind === 'redis' ? 'INFO keyspace\n' : undefined;
      const path = await dbApi.createQuery(root, name, kind, { connection: selected.slug, text });
      await load();
      const entry = (await dbApi.queries(root)).find((q) => q.path === path);
      if (entry) onOpen(entry);
    } catch (e) {
      toast.error('Could not create the query', errorMessage(e));
    }
  };

  const remove = async (q: SavedQueryEntry) => {
    if (!(await confirmDialog({ title: `Delete “${q.name}”?`, message: `queries/${q.path} moves to your system trash.`, confirmText: 'Move to trash', danger: true }))) return;
    try {
      await dbApi.deleteQuery(root, q.path);
      await load();
    } catch (e) {
      toast.error('Delete failed', errorMessage(e));
    }
  };

  return (
    <div className={clsx('db-saved', open && 'open')}>
      <div className="list-group-header db-saved-head" onClick={() => setOpen((o) => !o)}>
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        <span className="grow">Saved queries</span>
        <button
          className={clsx('btn btn-sm btn-ghost db-saved-toggle', all && 'active')}
          onClick={(e) => {
            e.stopPropagation();
            setAll((a) => !a);
          }}
          title={all ? 'Showing all connections' : 'Showing this connection'}
        >
          {all ? 'All' : 'This'}
        </button>
        <button
          className="icon-btn sm"
          onClick={(e) => {
            e.stopPropagation();
            void create();
          }}
          disabled={!selected}
          title="New saved query for this connection"
          aria-label="New saved query"
        >
          <Plus />
        </button>
      </div>
      {open && (
        <div className="list db-saved-list">
          {shown.length === 0 && <div className="subtle small px-4 p-2">{selected ? 'None yet — Save as… in an editor' : 'Select a connection'}</div>}
          {shown.map((q) => (
            <div
              key={q.path}
              className="list-item"
              onClick={() => onOpen(q)}
              tabIndex={0}
              onKeyDown={(e) => e.key === 'Enter' && onOpen(q)}
              onContextMenu={(e) =>
                ctx.open(e, [
                  { label: 'Open', onSelect: () => onOpen(q) },
                  { label: 'Reveal file', icon: <FolderOpen />, onSelect: () => void revealItemInDir(`${root}/queries/${q.path}`).catch((err) => toast.error('Could not reveal', errorMessage(err))) },
                  { label: 'Local history…', icon: <History />, onSelect: () => openLocalHistory({ root, path: `queries/${q.path}` }) },
                  { separator: true },
                  { label: 'Delete', icon: <Trash2 />, danger: true, onSelect: () => void remove(q) },
                ])
              }
              title={q.error ?? `queries/${q.path}${q.connection ? ` · ${q.connection}` : ''}`}
            >
              {q.error ? <AlertTriangle size={13} className="text-warning" /> : ICON[q.kind]}
              <span className="truncate grow">{q.path.includes('/') ? q.path.replace(/\.(sql|redis|mongo\.json)$/, '') : q.name}</span>
              {all && q.connection && <span className="subtle small truncate">{q.connection}</span>}
            </div>
          ))}
        </div>
      )}
      {ctx.node}
    </div>
  );
}
