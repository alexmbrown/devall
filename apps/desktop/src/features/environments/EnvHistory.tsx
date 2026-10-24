import { useEffect, useMemo, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { History as HistoryIcon, KeyRound, Link2, RotateCcw } from 'lucide-react';
import { CodeDiff } from '@/components/editor/CodeEditor';
import { Modal } from '@/components/ui/Modal';
import { Empty, RelTime, Segmented, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { historyApi, useLocalHistoryVersion, versionLabel, type FileVersion } from '@/lib/api/history';
import { SourceIcon, restoreVersion } from '@/features/history/FileHistoryView';
import { diffEnv, summarizeChanges, type EnvChange, type EnvLike } from './envDiff';
import '@/features/history/history.css';

type Mode = 'previous' | 'current' | 'raw';

interface Loaded {
  version: FileVersion;
  parsed: EnvLike | null;
}

const LABEL: Record<string, string> = { name: 'Name', color: 'Colour', label: 'Badge', description: 'Description', protected: 'Protected' };

function ChangeRow({ c }: { c: EnvChange }) {
  const sign = c.kind === 'added' ? '+' : c.kind === 'removed' ? '−' : '~';
  let name: ReactNode;
  let value: ReactNode;
  if (c.target === 'variable') {
    name = <code className="mono xs">{c.name}</code>;
    value = c.kind === 'changed' ? (
      <span className="mono xs">
        <del>{c.from || '(empty)'}</del> → <ins>{c.to || '(empty)'}</ins>
      </span>
    ) : (
      <span className="mono xs">{c.value || '(empty)'}</span>
    );
  } else if (c.target === 'secret') {
    name = (
      <span className="row-sm">
        <KeyRound size={12} /> <code className="mono xs">{`secret.${c.name}`}</code>
      </span>
    );
    value = <span className="xs subtle">secret {c.kind} (values stay in the keychain)</span>;
  } else if (c.target === 'secretSource') {
    name = (
      <span className="row-sm">
        <Link2 size={12} /> <code className="mono xs">{c.name}</code>
      </span>
    );
    value =
      c.kind === 'changed' ? (
        <span className="mono xs">
          <del>{c.from}</del> → <ins>{c.to}</ins>
        </span>
      ) : (
        <span className="mono xs">
          {c.kind === 'added' ? 'linked to ' : 'unlinked from '}
          {c.value}
        </span>
      );
  } else if (c.target === 'protected') {
    name = <span className="small">{LABEL.protected}</span>;
    value = <span className="xs">{c.to ? 'turned on' : 'turned off'}</span>;
  } else {
    name = <span className="small">{LABEL[c.target]}</span>;
    value = (
      <span className="xs">
        <del>{c.from || '(none)'}</del> → <ins>{c.to || '(none)'}</ins>
      </span>
    );
  }
  return (
    <div className={clsx('env-change', c.kind)}>
      <span className="env-change-kind">{sign}</span>
      <span className="truncate">{name}</span>
      <span className="env-change-value">{value}</span>
    </div>
  );
}

/**
 * Versions of an environment file (or of `devall.yaml` for project variables) from local history,
 * with what each version changed per variable, a comparison with the current file and Restore.
 */
export function EnvHistoryDialog({ root, path, title, current, onClose }: { root: string; path: string; title: string; current: EnvLike; onClose(): void }) {
  const version = useLocalHistoryVersion((s) => s.files);
  const [items, setItems] = useState<Loaded[] | null>(null);
  const [sel, setSel] = useState<number | null>(null);
  const [mode, setMode] = useState<Mode>('previous');
  const [raw, setRaw] = useState<{ a: string; b: string }>({ a: '', b: '' });

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const versions = await historyApi.fileVersions(root, path, 100);
        const parsed = await Promise.all(versions.map((v) => (v.deleted ? Promise.resolve(null) : historyApi.yaml<EnvLike>(root, v.id).catch(() => null))));
        if (alive) setItems(versions.map((v, i) => ({ version: v, parsed: parsed[i] })));
      } catch (e) {
        if (alive) setItems([]);
        toast.error('Local history', errorMessage(e));
      }
    })();
    return () => {
      alive = false;
    };
  }, [root, path, version]);

  useEffect(() => {
    if (!items?.length || (sel !== null && items.some((x) => x.version.id === sel))) return;
    setSel(items[0].version.id);
  }, [items, sel]);

  const idx = items?.findIndex((x) => x.version.id === sel) ?? -1;
  const cur = idx >= 0 ? items![idx] : null;
  const prev = idx >= 0 ? (items![idx + 1] ?? null) : null;
  const summaries = useMemo(() => items?.map((x, i) => summarizeChanges(diffEnv(items[i + 1]?.parsed ?? null, x.parsed))) ?? [], [items]);
  const changes = useMemo(() => {
    if (!cur) return [];
    // "vs current": what restoring would do to the file as it is now.
    return mode === 'current' ? diffEnv(current, cur.parsed) : diffEnv(prev?.parsed ?? null, cur.parsed);
  }, [cur, prev, mode, current]);

  useEffect(() => {
    if (mode !== 'raw' || !cur) return;
    let alive = true;
    Promise.all([prev && !prev.version.deleted ? historyApi.content(root, prev.version.id) : Promise.resolve(''), cur.version.deleted ? Promise.resolve('') : historyApi.content(root, cur.version.id)])
      .then(([a, b]) => alive && setRaw({ a: a ?? '', b: b ?? '' }))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [mode, root, cur?.version.id, prev?.version.id]);

  return (
    <Modal
      open
      onClose={onClose}
      size="xl"
      bodyClassName="lh-modal-body"
      title={
        <span className="row-sm">
          <HistoryIcon size={16} /> History — {title}
        </span>
      }
      headerExtra={
        <span className="xs subtle mono truncate lh-path" title={path}>
          {path}
        </span>
      }
    >
      {!items ? (
        <div className="empty">
          <Spinner />
        </div>
      ) : !items.length ? (
        <Empty icon={<HistoryIcon />} title="No history yet">
          Versions are kept on this computer each time the file changes — saved here, edited elsewhere or changed by git.
        </Empty>
      ) : (
        <div className="lh-split">
          <div className="lh-list list" role="listbox" aria-label="Versions">
            {items.map((x, i) => (
              <div key={x.version.id} className={clsx('list-item lh-version', sel === x.version.id && 'on')} onClick={() => setSel(x.version.id)} role="option" aria-selected={sel === x.version.id} tabIndex={0}>
                <SourceIcon v={x.version} />
                <div className="grow lh-version-text">
                  <div className="small truncate">
                    <RelTime ts={x.version.at} />
                    {i === 0 && !x.version.deleted && <span className="badge outline lh-badge">current</span>}
                  </div>
                  <div className="xs subtle truncate" title={`${versionLabel(x.version)} · ${new Date(x.version.at).toLocaleString()}`}>
                    {x.version.deleted ? 'Deleted' : i === items.length - 1 ? versionLabel(x.version) : summaries[i]}
                  </div>
                </div>
              </div>
            ))}
          </div>
          <div className="lh-detail">
            {cur && (
              <>
                <div className="toolbar">
                  <span className="xs subtle truncate grow">
                    {new Date(cur.version.at).toLocaleString()} · {versionLabel(cur.version)}
                  </span>
                  <Segmented<Mode>
                    value={mode}
                    onChange={setMode}
                    options={[
                      { value: 'previous', label: 'Changes', title: 'What this version changed' },
                      { value: 'current', label: 'vs current', title: 'What restoring this version would change' },
                      { value: 'raw', label: 'File diff' },
                    ]}
                  />
                  <button className="btn btn-sm btn-primary" disabled={cur.version.deleted || idx === 0} onClick={() => void restoreVersion(root, cur.version).then((ok) => ok && onClose())}>
                    <RotateCcw /> Restore this version
                  </button>
                </div>
                {mode === 'raw' ? (
                  <div className="grow min-h-0">
                    <CodeDiff original={raw.a} modified={raw.b} language="yaml" />
                  </div>
                ) : cur.version.deleted ? (
                  <Empty title="The file was deleted here" />
                ) : !cur.parsed ? (
                  <Empty title="This version isn't valid YAML">Use File diff to see it.</Empty>
                ) : changes.length === 0 ? (
                  <Empty title={mode === 'current' ? 'Same as the current file' : 'No variable changes'}>{mode === 'previous' ? 'Formatting or comments changed — see File diff.' : null}</Empty>
                ) : (
                  <div className="env-hist-changes">
                    {mode === 'current' && <p className="xs subtle">Restoring would change the current file like this:</p>}
                    {changes.map((c, i) => (
                      <ChangeRow key={i} c={c} />
                    ))}
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      )}
    </Modal>
  );
}
