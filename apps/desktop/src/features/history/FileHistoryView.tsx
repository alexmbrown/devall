import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { Bot, FilePen, FileX2, GitCompare, History as HistoryIcon, Pencil, RotateCcw, Sparkles, Trash2 } from 'lucide-react';
import { CodeDiff, CodeEditor, languageFor, type CodeLanguage } from '@/components/editor/CodeEditor';
import { useContextMenu } from '@/components/ui/Menu';
import { Empty, RelTime, Segmented, Spinner, formatBytes } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import { historyApi, useLocalHistoryVersion, versionLabel, type FileVersion, type VersionSource } from '@/lib/api/history';
import { useVarsVersion } from '@/store/variables';
import { useWorkspace } from '@/store/workspace';
import './history.css';

/** Monaco language for a project file. */
export function languageForPath(path: string, text?: string): CodeLanguage {
  const p = path.toLowerCase();
  if (p.endsWith('.md')) return 'markdown';
  if (p.endsWith('.ipynb')) return 'json';
  if (p.endsWith('.sql')) return 'sql';
  if (p.endsWith('.graphql') || p.endsWith('.gql')) return 'graphql';
  if (p.endsWith('.csv') || p.endsWith('.txt')) return 'plaintext';
  return languageFor(p, text);
}

export function SourceIcon({ v }: { v: Pick<FileVersion, 'source' | 'deleted'> }) {
  if (v.deleted) return <FileX2 className="lh-src danger" aria-hidden />;
  const icons: Record<VersionSource, React.ReactNode> = {
    initial: <Sparkles className="lh-src" aria-hidden />,
    external: <FilePen className="lh-src warning" aria-hidden />,
    app: <Pencil className="lh-src accent" aria-hidden />,
    agent: <Bot className="lh-src accent" aria-hidden />,
    restore: <RotateCcw className="lh-src success" aria-hidden />,
  };
  return <>{icons[v.source]}</>;
}

/** After a restore: reload what shows the file (tree, environments, variables). */
export async function afterRestore(path: string) {
  useLocalHistoryVersion.getState().bumpFiles();
  if (path === 'devall.yaml' || path.startsWith('http/') || path.startsWith('environments/')) {
    await useWorkspace.getState().refresh();
    useVarsVersion.getState().bump();
  }
}

/** Restores a version after confirming. Returns true when restored. */
export async function restoreVersion(root: string, v: FileVersion): Promise<boolean> {
  const ok = await confirmDialog({
    title: 'Restore this version?',
    message: `${v.path} is replaced with the version from ${new Date(v.at).toLocaleString()}. The current content stays in local history, so you can go back.`,
    confirmText: 'Restore',
  });
  if (!ok) return false;
  try {
    await historyApi.restore(root, v.id);
    await afterRestore(v.path);
    toast.success('Version restored', v.path);
    return true;
  } catch (e) {
    toast.error('Could not restore', errorMessage(e));
    return false;
  }
}

type Mode = 'current' | 'previous' | 'content';

/**
 * Versions of one project file (`path` relative to the project root) with a diff against the
 * current file or the previous version, and Restore. Used by the "Local history…" dialog and the
 * History page.
 */
export function FileHistoryView({ root, path, onRestored }: { root: string; path: string; onRestored?(): void }) {
  const version = useLocalHistoryVersion((s) => s.files);
  const [versions, setVersions] = useState<FileVersion[] | null>(null);
  const [sel, setSel] = useState<number | null>(null);
  const [mode, setMode] = useState<Mode>('current');
  const [current, setCurrent] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [previous, setPrevious] = useState<string | null>(null);
  const ctx = useContextMenu();

  useEffect(() => {
    let alive = true;
    historyApi
      .fileVersions(root, path)
      .then((v) => alive && setVersions(v))
      .catch((e) => {
        if (alive) setVersions([]);
        toast.error('Local history', errorMessage(e));
      });
    api
      .readTextFile(`${root}/${path}`)
      .then((t) => alive && setCurrent(t))
      .catch(() => alive && setCurrent(null));
    return () => {
      alive = false;
    };
  }, [root, path, version]);

  // Default: the newest version that differs from what is on disk (usually the second one).
  useEffect(() => {
    if (!versions?.length) return;
    if (sel !== null && versions.some((v) => v.id === sel)) return;
    setSel((versions.length > 1 && !versions[0].deleted ? versions[1] : versions[0]).id);
  }, [versions, sel]);

  const idx = versions?.findIndex((v) => v.id === sel) ?? -1;
  const selV = idx >= 0 ? versions![idx] : null;
  const prevV = idx >= 0 ? (versions![idx + 1] ?? null) : null;

  useEffect(() => {
    let alive = true;
    if (!selV) return setSelected(null);
    historyApi
      .content(root, selV.id)
      .then((t) => alive && setSelected(t))
      .catch(() => alive && setSelected(null));
    return () => {
      alive = false;
    };
  }, [root, selV?.id]);
  useEffect(() => {
    let alive = true;
    if (!prevV) return setPrevious(null);
    historyApi
      .content(root, prevV.id)
      .then((t) => alive && setPrevious(t))
      .catch(() => alive && setPrevious(null));
    return () => {
      alive = false;
    };
  }, [root, prevV?.id]);

  const lang = useMemo(() => languageForPath(path, selected ?? current ?? undefined), [path, selected, current]);

  if (!versions)
    return (
      <div className="empty">
        <Spinner />
      </div>
    );
  if (!versions.length)
    return (
      <Empty icon={<HistoryIcon />} title="No local history yet">
        Versions are recorded on this computer when the file changes — saved here, edited elsewhere, or changed by git. Nothing is written to the project.
      </Empty>
    );

  const restore = async (v: FileVersion) => {
    if (await restoreVersion(root, v)) onRestored?.();
  };
  const remove = async (v: FileVersion) => {
    try {
      await historyApi.deleteVersion(root, v.id);
      useLocalHistoryVersion.getState().bumpFiles();
    } catch (e) {
      toast.error('Could not delete the version', errorMessage(e));
    }
  };

  return (
    <div className="lh-split">
      <div className="lh-list list" role="listbox" aria-label="Versions">
        {versions.map((v, i) => (
          <div
            key={v.id}
            className={clsx('list-item lh-version', sel === v.id && 'on')}
            onClick={() => setSel(v.id)}
            onContextMenu={(e) =>
              ctx.open(e, [
                { label: 'Restore this version', icon: <RotateCcw />, disabled: v.deleted || i === 0, onSelect: () => void restore(v) },
                { separator: true },
                { label: 'Delete this version', icon: <Trash2 />, danger: true, onSelect: () => void remove(v) },
              ])
            }
            role="option"
            aria-selected={sel === v.id}
            tabIndex={0}
            onKeyDown={(e) => {
              if (e.key === 'ArrowDown' && versions[i + 1]) setSel(versions[i + 1].id);
              if (e.key === 'ArrowUp' && versions[i - 1]) setSel(versions[i - 1].id);
            }}
          >
            <SourceIcon v={v} />
            <div className="grow lh-version-text">
              <div className="small truncate">
                <RelTime ts={v.at} />
                {i === 0 && !v.deleted && <span className="badge outline lh-badge">current</span>}
              </div>
              <div className="xs subtle truncate" title={new Date(v.at).toLocaleString()}>
                {versionLabel(v)}
                {!v.deleted && ` · ${formatBytes(v.size)}`}
              </div>
            </div>
          </div>
        ))}
        {ctx.node}
      </div>
      <div className="lh-detail">
        {selV && (
          <>
            <div className="toolbar">
              <span className="xs subtle truncate grow" title={new Date(selV.at).toLocaleString()}>
                {new Date(selV.at).toLocaleString()} · {versionLabel(selV)}
              </span>
              <Segmented<Mode>
                value={mode}
                onChange={setMode}
                options={[
                  { value: 'current', label: <><GitCompare size={12} /> vs current</>, title: 'What changed between this version and the file now' },
                  { value: 'previous', label: 'vs previous', title: 'What this version changed' },
                  { value: 'content', label: 'Content' },
                ]}
              />
              <button className="btn btn-sm btn-primary" disabled={selV.deleted || idx === 0} onClick={() => void restore(selV)} title={idx === 0 ? 'This is the current version' : 'Write this version back to the file'}>
                <RotateCcw /> Restore
              </button>
            </div>
            <div className="grow min-h-0">
              {selV.deleted ? (
                <Empty icon={<FileX2 />} title="The file was deleted here">
                  Pick an earlier version to see or restore its content.
                </Empty>
              ) : mode === 'content' ? (
                <CodeEditor value={selected ?? ''} language={lang} readOnly />
              ) : mode === 'current' ? (
                <CodeDiff original={selected ?? ''} modified={current ?? ''} language={lang} />
              ) : (
                <CodeDiff original={previous ?? ''} modified={selected ?? ''} language={lang} />
              )}
            </div>
            {mode === 'current' && current === null && !selV.deleted && <div className="xs subtle lh-note">The file no longer exists — restoring brings it back.</div>}
          </>
        )}
      </div>
    </div>
  );
}
