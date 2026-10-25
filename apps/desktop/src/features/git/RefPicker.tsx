import { useEffect, useMemo, useRef, useState } from 'react';
import { create } from 'zustand';
import clsx from 'clsx';
import { Check, GitBranch, GitCommitHorizontal, Globe, Tag } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { RelTime, SearchInput, Spinner } from '@/components/ui/misc';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type RefEntry, type RefKind, type RefList } from '@/lib/api/git';

/**
 * "Pick a branch, tag or commit": every local branch, remote branch and tag (newest first) with
 * search; anything else typed (a SHA, `HEAD~2`) can be used as it is. `pickRef()` resolves with
 * the name, or null when cancelled.
 */
interface Pending {
  root: string;
  repo: string | null;
  title: string;
  current?: string | null;
  resolve(v: string | null): void;
}

const usePicker = create<{ pending: Pending | null }>(() => ({ pending: null }));

export function pickRef(opts: Omit<Pending, 'resolve'>): Promise<string | null> {
  return new Promise((resolve) => usePicker.setState({ pending: { ...opts, resolve } }));
}

const ICON: Record<RefKind, React.ReactNode> = { branch: <GitBranch size={13} />, remote: <Globe size={13} />, tag: <Tag size={13} />, commit: <GitCommitHorizontal size={13} /> };
const GROUP: Record<RefKind, string> = { branch: 'Branches', remote: 'Remote branches', tag: 'Tags', commit: 'Commits' };
const looksLikeRev = (s: string) => /^[^\s~^:?*[\\-][^\s:?*[\\]*$/.test(s) && !s.includes('..');

export function RefPickerHost() {
  const pending = usePicker((s) => s.pending);
  if (!pending) return null;
  const close = (v: string | null) => {
    usePicker.setState({ pending: null });
    pending.resolve(v);
  };
  return <RefPickerDialog p={pending} close={close} />;
}

function RefPickerDialog({ p, close }: { p: Pending; close(v: string | null): void }) {
  const [refs, setRefs] = useState<RefList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [q, setQ] = useState('');
  const [hl, setHl] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    gitApi.refs(p.root, p.repo).then(setRefs).catch((e) => setError(errorMessage(e)));
  }, [p.root, p.repo]);

  const items = useMemo(() => {
    const terms = q.toLowerCase().split(/\s+/).filter(Boolean);
    const list: RefEntry[] = (refs?.refs ?? []).filter((r) => terms.every((t) => r.name.toLowerCase().includes(t) || r.subject.toLowerCase().includes(t) || r.sha.startsWith(t)));
    const order: RefKind[] = ['branch', 'remote', 'tag'];
    list.sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind));
    const typed = q.trim();
    const extra: RefEntry[] = typed && looksLikeRev(typed) && !list.some((r) => r.name === typed) ? [{ name: typed, refname: '', sha: '', kind: 'commit', upstream: null, track: null, ahead: 0, behind: 0, gone: false, worktree: null, annotated: false, time: 0, subject: 'Use as typed (commit, tag or revision)', current: false }] : [];
    if (!terms.length && refs?.detached) extra.push({ name: 'HEAD', refname: 'HEAD', sha: refs.head ?? '', kind: 'commit', upstream: null, track: null, ahead: 0, behind: 0, gone: false, worktree: null, annotated: false, time: 0, subject: 'The checked-out commit (detached)', current: true });
    return [...extra, ...list].slice(0, 400);
  }, [refs, q]);

  useEffect(() => setHl(0), [q]);
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-i="${hl}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [hl]);

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setHl((h) => Math.min(items.length - 1, h + 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setHl((h) => Math.max(0, h - 1));
    } else if (e.key === 'Enter' && items[hl]) {
      e.preventDefault();
      close(items[hl].name);
    }
  };

  let lastGroup = '';
  return (
    <Modal open onClose={() => close(null)} title={p.title} size="lg" bodyClassName="git-refpick-body">
      <div className="col" onKeyDown={onKey}>
        <SearchInput value={q} onChange={setQ} placeholder="Branch, tag or commit…" autoFocus />
        {error && <div className="small git-fail">{error}</div>}
        <div className="git-refpick-list" ref={listRef}>
          {!refs && !error && (
            <div className="p-5 row-sm subtle small">
              <Spinner /> Reading refs…
            </div>
          )}
          {items.map((r, i) => {
            const group = r.kind === 'commit' ? '' : GROUP[r.kind];
            const head = group !== lastGroup ? group : '';
            lastGroup = group;
            return (
              <div key={`${r.kind}:${r.name}`}>
                {head && <div className="menu-label">{head}</div>}
                <div data-i={i} className={clsx('list-item git-refpick-item', i === hl && 'on')} onMouseEnter={() => setHl(i)} onClick={() => close(r.name)}>
                  <span className="subtle shrink-0">{ICON[r.kind]}</span>
                  <span className="mono small truncate">{r.name}</span>
                  {r.current && <Check size={13} className="git-ok shrink-0" />}
                  {p.current === r.name && <span className="badge outline shrink-0">selected</span>}
                  <span className="xs subtle truncate grow" title={r.subject}>
                    {r.subject}
                  </span>
                  {r.time > 0 && (
                    <span className="xs subtle shrink-0">
                      <RelTime ts={r.time * 1000} />
                    </span>
                  )}
                </div>
              </div>
            );
          })}
          {refs && !items.length && <div className="p-5 subtle small">Nothing matches. Type a commit SHA or revision to use it as is.</div>}
        </div>
      </div>
    </Modal>
  );
}
