import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { Ban, BookOpen, Cable, Ear, FileCode, FolderOpen, Heading, KeyRound, ListTree, MessageSquare, MoreHorizontal, Plug, Save, Send, Settings2, Tags, Unplug } from 'lucide-react';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Spinner } from '@/components/ui/misc';
import { ResponsiveTabs } from '@/components/ui/ResponsiveTabs';
import { LayoutToggle, ReqResSplit } from '@/components/layout/ReqResSplit';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { protoApi, type ProtoFile, type ProtoKind } from '@/lib/api/proto';
import { useActiveViewRef, useKeepView } from '@/store/nav';
import { usePrefs } from '@/store/prefs';
import { useWorkspace } from '@/store/workspace';
import { liveKey, useLive, type LiveStatus } from './store';
import './proto.css';

const AUTOSAVE_MS = 600;

export const KIND_LABEL: Record<ProtoKind, string> = { ws: 'WebSocket', grpc: 'gRPC', mqtt: 'MQTT', socketio: 'Socket.IO' };
export const KIND_BADGE: Record<ProtoKind, string> = { ws: 'WS', grpc: 'GRPC', mqtt: 'MQTT', socketio: 'SIO' };

export function KindBadge({ kind }: { kind: ProtoKind }) {
  return (
    <span className={clsx('badge proto-kind', kind)} title={KIND_LABEL[kind]}>
      {KIND_BADGE[kind]}
    </span>
  );
}

/**
 * Loads a protocol file and keeps an editable draft: autosaves (debounced, honouring the
 * auto-save preference), saves on demand, flushes pending edits on unmount and reloads when the
 * file changed on disk while clean.
 */
export function useProtoFile<T extends ProtoFile>(root: string, path: string) {
  const [draft, setDraft] = useState<T | null>(null);
  const [saved, setSaved] = useState('');
  const [error, setError] = useState<string>();
  const autoSave = usePrefs((s) => s.autoSave);
  const tree = useWorkspace((s) => s.tree);
  const draftRef = useRef<T | null>(null);
  const savedRef = useRef('');
  draftRef.current = draft;
  savedRef.current = saved;
  const dirty = draft !== null && JSON.stringify(draft) !== saved;

  useEffect(() => {
    let live = true;
    setDraft(null);
    setError(undefined);
    protoApi
      .read<T>(root, path)
      .then((d) => {
        if (!live) return;
        setDraft(d);
        setSaved(JSON.stringify(d));
      })
      .catch((e) => live && setError(errorMessage(e)));
    return () => {
      live = false;
      // Flush unsaved edits when the editor goes away (tab switch/close).
      const d = draftRef.current;
      if (d && JSON.stringify(d) !== savedRef.current) protoApi.save(root, path, d).catch(() => {});
    };
  }, [root, path]);

  // Reload from disk when the tree refreshes (e.g. after `git pull`) and there are no local edits.
  useEffect(() => {
    if (!draftRef.current || JSON.stringify(draftRef.current) !== savedRef.current) return;
    protoApi
      .read<T>(root, path)
      .then((fresh) => {
        const s = JSON.stringify(fresh);
        if (s !== savedRef.current && JSON.stringify(draftRef.current) === savedRef.current) {
          setDraft(fresh);
          setSaved(s);
        }
      })
      .catch(() => {});
  }, [tree, root, path]);

  const save = useCallback(async () => {
    const d = draftRef.current;
    if (!d) return false;
    try {
      await protoApi.save(root, path, d);
      setSaved(JSON.stringify(d));
      return true;
    } catch (e) {
      toast.error('Save failed', errorMessage(e));
      return false;
    }
  }, [root, path]);

  useEffect(() => {
    if (!dirty || !autoSave) return;
    const t = setTimeout(save, AUTOSAVE_MS);
    return () => clearTimeout(t);
  }, [draft, dirty, autoSave, save]);

  // The first edit turns a preview tab into a normal one.
  const keepView = useKeepView();
  const update = useCallback(
    (fn: (d: T) => T) => {
      keepView();
      setDraft((d) => (d ? fn(d) : d));
    },
    [keepView],
  );
  return { draft, update, dirty, save, error };
}

/** Ctrl/Cmd+S saves, Ctrl/Cmd+Enter runs the primary action — scoped to the mounted editor. */
export function useEditorKeys(onSave: () => void, onRun: () => void) {
  const ref = useRef({ onSave, onRun });
  ref.current = { onSave, onRun };
  // Two editors can be visible side by side: only the active view's reacts.
  const active = useActiveViewRef();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || !active.current) return;
      if (e.key === 'Enter') {
        e.preventDefault();
        ref.current.onRun();
      } else if (e.key.toLowerCase() === 's') {
        e.preventDefault();
        ref.current.onSave();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [active]);
}

const STATUS_TEXT: Record<LiveStatus, string> = { idle: 'Not connected', connecting: 'Connecting…', open: 'Connected', closed: 'Disconnected' };

export function StatusPill({ status, text }: { status: LiveStatus; text?: string }) {
  return (
    <span className="proto-status" role="status">
      {status === 'connecting' ? <Spinner /> : <span className={clsx('dot', status === 'open' && 'ok pulse', status === 'closed' && 'warn')} />}
      {text ?? STATUS_TEXT[status]}
    </span>
  );
}

export function ConnectButton({ status, onConnect, onDisconnect, connectLabel = 'Connect' }: { status: LiveStatus; onConnect(): void; onDisconnect(): void; connectLabel?: string }) {
  if (status === 'open')
    return (
      <button className="btn btn-send" onClick={onDisconnect} title="Disconnect">
        <Unplug /> Disconnect
      </button>
    );
  if (status === 'connecting')
    return (
      <button className="btn btn-send" onClick={onDisconnect} title="Cancel">
        <Ban /> Cancel
      </button>
    );
  return (
    <button className="btn btn-primary btn-send" onClick={onConnect} title={`${connectLabel} (Ctrl+Enter)`}>
      <Plug /> {connectLabel}
    </button>
  );
}

/** Name (rename on blur), path, save button and the "more" menu. */
export function ProtoHead({
  root,
  path,
  name,
  dirty,
  onSave,
  status,
  menu = [],
}: {
  root: string;
  path: string;
  name: string;
  dirty: boolean;
  onSave(): Promise<boolean>;
  status: ReactNode;
  menu?: MenuItemDef[];
}) {
  const [value, setValue] = useState(name);
  useEffect(() => setValue(name), [name, path]);
  const rename = async () => {
    const next = value.trim();
    if (!next || next === name) return setValue(name);
    // The rename rewrites the file from disk: flush edits first.
    if (dirty && !(await onSave())) return;
    const to = await useWorkspace.getState().rename(path, next);
    if (to) useLive.getState().rekey(liveKey(root, path), liveKey(root, to));
  };
  return (
    <div className="request-head">
      <input
        className="name-input"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onBlur={rename}
        onKeyDown={(e) => {
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
          if (e.key === 'Escape') {
            setValue(name);
            (e.target as HTMLInputElement).blur();
          }
        }}
        aria-label="Name"
      />
      <span className="subtle xs mono truncate req-path" title={`${root}/http/${path}`}>
        http/{path}
      </span>
      <span className="spacer" />
      {status}
      {dirty && (
        <button className="btn btn-sm" onClick={() => onSave()} title="Save (Ctrl+S)">
          <Save /> Save
        </button>
      )}
      <LayoutToggle />
      <Menu
        align="right"
        items={[
          ...menu,
          ...(menu.length ? [{ separator: true }] : []),
          { label: 'Reveal file', icon: <FolderOpen />, onSelect: () => revealItemInDir(`${root}/http/${path}`).catch((e) => toast.error('Could not reveal', String(e))) },
        ]}
        trigger={(p) => (
          <button className="icon-btn" {...p} aria-label="More actions">
            <MoreHorizontal />
          </button>
        )}
      />
    </div>
  );
}

/** Loading / error placeholder while the file is read. */
export function LoadState({ error }: { error?: string }) {
  if (error)
    return (
      <div className="p-6">
        <div className="callout danger">
          <div className="grow">
            <div className="strong">Could not open this file</div>
            <div className="mono xs break">{error}</div>
          </div>
        </div>
      </div>
    );
  return (
    <div className="p-6">
      <Spinner />
    </div>
  );
}

export interface SubTab<S extends string> {
  key: S;
  label: string;
  count?: number;
  dot?: boolean;
  /** Shown when the row is too narrow for labels (defaults by key). */
  icon?: ReactNode;
}

/** Icons of the sub-tabs the protocol editors use. */
const SUB_ICONS: Record<string, ReactNode> = {
  message: <MessageSquare />,
  headers: <Heading />,
  metadata: <Tags />,
  settings: <Settings2 />,
  docs: <BookOpen />,
  auth: <KeyRound />,
  connection: <Cable />,
  emit: <Send />,
  publish: <Send />,
  listen: <Ear />,
  subscriptions: <Ear />,
  schema: <FileCode />,
};

/** Shared layout: head, URL bar, sub-tabbed request pane on the left, live log on the right. */
export function EditorFrame<S extends string>({
  head,
  bar,
  tabs,
  sub,
  onSub,
  children,
  log,
}: {
  head: ReactNode;
  bar: ReactNode;
  tabs: SubTab<NoInfer<S>>[];
  sub: S;
  onSub(s: NoInfer<S>): void;
  children: ReactNode;
  log: ReactNode;
}) {
  return (
    <div className="request-editor">
      {head}
      <div className="url-bar">{bar}</div>
      <ReqResSplit
        request={
          <>
            <ResponsiveTabs
              tabs={tabs.map((t) => ({ key: t.key, label: t.label, icon: t.icon ?? SUB_ICONS[t.key] ?? <ListTree />, badge: t.count || undefined, dot: t.dot }))}
              value={sub}
              onChange={onSub}
            />
            <div className="request-sub">{children}</div>
          </>
        }
        response={log}
      />
    </div>
  );
}

const sticky = new Map<string, unknown>();

/** useState that survives unmounts (tab switches) for the app session, keyed by `key`. */
export function useSticky<T>(key: string, initial: () => T): [T, (v: T | ((p: T) => T)) => void] {
  const [value, setValue] = useState<T>(() => (sticky.has(key) ? (sticky.get(key) as T) : initial()));
  useEffect(() => {
    setValue(sticky.has(key) ? (sticky.get(key) as T) : initial());
  }, [key]); // `initial` only matters for a new key
  const set = useCallback(
    (v: T | ((p: T) => T)) =>
      setValue((prev) => {
        const next = typeof v === 'function' ? (v as (p: T) => T)(prev) : v;
        sticky.set(key, next);
        return next;
      }),
    [key],
  );
  return [value, set];
}

/** Asks before connecting while a protected environment is active. */
export async function confirmProtected(root: string): Promise<boolean> {
  const p = useWorkspace.getState().project;
  const envId = usePrefs.getState().activeEnv[root];
  const env = p?.environments.find((e) => e.id === envId);
  if (!env?.file.protected) return true;
  return confirmDialog({
    title: `Connect while “${env.file.name}” is active?`,
    message: `“${env.file.name}” is a protected environment — messages you send go to it.`,
    confirmText: 'Connect',
    danger: true,
  });
}

/** Comma-separated list input that keeps the user's typing and commits clean, non-empty items. */
export function CommaListInput({ value, onChange, placeholder }: { value: string[]; onChange(v: string[]): void; placeholder?: string }) {
  const [text, setText] = useState(value.join(', '));
  const joined = value.join(', ');
  useEffect(() => {
    // External changes (reload from disk) replace the text unless it already means the same.
    setText((t) => (split(t).join(', ') === joined ? t : joined));
  }, [joined]);
  return (
    <input
      className="input mono"
      value={text}
      placeholder={placeholder}
      onChange={(e) => {
        setText(e.target.value);
        onChange(split(e.target.value));
      }}
    />
  );
}

function split(s: string): string[] {
  return s
    .split(',')
    .map((x) => x.trim())
    .filter(Boolean);
}

/** "1 400" → 1400; empty or invalid → fallback. */
export function num(v: string, fallback = 0): number {
  const n = Number(v.replace(/\s/g, ''));
  return Number.isFinite(n) && n >= 0 ? Math.floor(n) : fallback;
}
