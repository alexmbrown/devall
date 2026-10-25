/**
 * Repository dialogs: "Repositories" (add / rename / group / locate / clone / remove — the list in
 * devall.yaml plus this machine's locations), "Switch branch" across repos, clone progress and
 * bulk results. One host (mounted in App) so the project menu, the Projects page and Source
 * control open the same dialogs.
 */
import { useEffect, useMemo, useState } from 'react';
import { create } from 'zustand';
import clsx from 'clsx';
import { CheckCircle2, Download, FolderOpen, FolderSearch, GitBranch, MoreHorizontal, Pencil, Plus, Save, Tag, Trash2, XCircle } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { Modal } from '@/components/ui/Modal';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Callout, Checkbox, Empty, Field, Segmented, Spinner } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, reposApi, type BulkResult, type Repo, type RepoBranches, type RepoSuggestion } from '@/lib/api/git';
import { useRepos } from '@/store/repos';
import { useWorkspace } from '@/store/workspace';
import { branchUnion, bulkSummary, groupRepos, switchPlan, type SwitchPlan } from './model';
import './git.css';

type Dialog = { kind: 'repos' } | { kind: 'switch'; repos?: string[] } | { kind: 'results'; title: string; results: BulkResult[] } | { kind: 'clone'; repo: string } | null;

export const useGitDialogs = create<{ dialog: Dialog; open(d: Dialog): void; close(): void }>((set) => ({
  dialog: null,
  open: (dialog) => set({ dialog }),
  close: () => set({ dialog: null }),
}));

/** Opens the "Repositories" manager (project menu, Projects page, Source control). */
export const openReposDialog = () => useGitDialogs.getState().open({ kind: 'repos' });
/** "Switch branch…" across repositories (pre-selecting `repos`, else every present one). */
export const openSwitchBranch = (repos?: string[]) => useGitDialogs.getState().open({ kind: 'switch', repos });
/** Clones a missing listed repo from its remote, after confirming (progress in a dialog). */
export async function cloneRepo(r: Repo) {
  if (!r.remote) return toast.info(`${r.name} has no remote in devall.yaml`, 'Locate it on this machine instead, or add `remote:` to its entry.');
  const ok = await confirmDialog({ title: `Clone ${r.name}?`, message: <CloneSummary repo={r} />, confirmText: 'Clone' });
  if (ok) useGitDialogs.getState().open({ kind: 'clone', repo: r.name });
}

function CloneSummary({ repo }: { repo: Repo }) {
  return (
    <span className="col-sm">
      <span className="mono xs">git clone {repo.remote}</span>
      <span className="small">
        into <span className="mono xs">{repo.path}</span>
      </span>
    </span>
  );
}

/** "Locate…": where a listed repo is on this machine (kept out of devall.yaml). */
export async function locateRepo(r: Repo) {
  const root = useWorkspace.getState().project?.root;
  if (!root) return;
  const dir = await openDialog({ directory: true, title: `Where is ${r.name} on this machine?` });
  if (typeof dir !== 'string') return;
  try {
    await reposApi.locate(root, r.name, dir);
    toast.success(`${r.name} located`, `${dir} (this machine only)`);
    await refreshAll();
  } catch (e) {
    toast.error(`Could not locate ${r.name}`, errorMessage(e));
  }
}

/** Lists discovered (nested, unlisted) repos in devall.yaml. */
export async function saveDiscovered(names: string[]) {
  const root = useWorkspace.getState().project?.root;
  if (!root || !names.length) return;
  try {
    await reposApi.saveDiscovered(root, names);
    toast.success(names.length === 1 ? `${names[0]} saved to devall.yaml` : `${names.length} repositories saved to devall.yaml`, 'Commit devall.yaml to share them with your team.');
    await refreshAll();
  } catch (e) {
    toast.error('Could not save to devall.yaml', errorMessage(e));
  }
}

/** Fetch all / Pull all over `names` after confirming; per-repo results in a dialog. */
export async function runBulk(action: 'fetch' | 'pull', names: string[]) {
  const root = useWorkspace.getState().project?.root;
  if (!root || !names.length) return;
  const verb = action === 'fetch' ? 'Fetch' : 'Pull';
  const ok = await confirmDialog({
    title: `${verb} ${names.length} ${names.length === 1 ? 'repository' : 'repositories'}?`,
    message: action === 'pull' ? `git pull --ff-only in ${names.join(', ')}. Repositories that can't fast-forward are left as they are.` : `git fetch --prune in ${names.join(', ')}.`,
    confirmText: verb,
  });
  if (!ok) return;
  toast.info(`${verb}ing ${names.length} ${names.length === 1 ? 'repository' : 'repositories'}…`);
  try {
    const results = await gitApi.bulk(root, names, action);
    const details = { kind: 'results' as const, title: `${verb} — ${bulkSummary(results)}`, results };
    if (results.every((r) => r.ok)) toast.success(`${verb} done`, bulkSummary(results), { label: 'Details', run: () => useGitDialogs.getState().open(details) });
    else useGitDialogs.getState().open(details);
  } catch (e) {
    toast.error(`${verb} failed`, errorMessage(e));
  }
  await useRepos.getState().refreshOverview(names).catch(() => {});
}

async function refreshAll() {
  await useRepos.getState().refreshOverview().catch(() => useRepos.getState().load().catch(() => {}));
}

export function GitDialogsHost() {
  const dialog = useGitDialogs((s) => s.dialog);
  const close = useGitDialogs((s) => s.close);
  const root = useWorkspace((s) => s.project?.root);
  if (!dialog || !root) return null;
  switch (dialog.kind) {
    case 'repos':
      return <ReposDialog root={root} onClose={close} />;
    case 'switch':
      return <SwitchBranchDialog root={root} initial={dialog.repos} onClose={close} />;
    case 'results':
      return (
        <Modal open onClose={close} title={dialog.title} size="lg" footer={<button className="btn btn-primary" onClick={close}>Close</button>}>
          <ResultsTable results={dialog.results} />
        </Modal>
      );
    case 'clone':
      return <CloneDialog root={root} name={dialog.repo} onClose={close} />;
  }
}

function ResultsTable({ results }: { results: BulkResult[] }) {
  return (
    <div className="git-table-wrap">
      <table className="table">
        <tbody>
          {results.map((r) => (
            <tr key={r.repo}>
              <td className="shrink-0">{r.ok ? <CheckCircle2 size={14} className="git-ok" /> : <XCircle size={14} className="git-fail" />}</td>
              <td className="strong">{r.repo}</td>
              <td>
                <pre className="git-output">{r.message || (r.ok ? 'done' : 'failed')}</pre>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ---------- clone ----------

function CloneDialog({ root, name, onClose }: { root: string; name: string; onClose(): void }) {
  const [lines, setLines] = useState<string[]>([]);
  const [state, setState] = useState<'running' | 'done' | 'failed'>('running');
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    reposApi
      .clone(root, name, (line) => alive && setLines((l) => (l.length && /\d+%/.test(line) && l[l.length - 1].split(':')[0] === line.split(':')[0] ? [...l.slice(0, -1), line] : [...l.slice(-200), line])))
      .then(() => {
        if (!alive) return;
        setState('done');
        toast.success(`${name} cloned`);
        void refreshAll();
      })
      .catch((e) => {
        if (!alive) return;
        setState('failed');
        setError(errorMessage(e));
      });
    return () => {
      alive = false;
    };
  }, [root, name]);
  return (
    <Modal
      open
      onClose={state === 'running' ? () => {} : onClose}
      title={<span className="row-sm">{state === 'running' ? <Spinner /> : <Download size={16} />} Clone {name}</span>}
      size="lg"
      footer={
        <button className="btn btn-primary" disabled={state === 'running'} onClick={onClose}>
          {state === 'running' ? 'Cloning…' : 'Close'}
        </button>
      }
    >
      <div className="col">
        {error && <Callout kind="danger">{error}</Callout>}
        {state === 'done' && <Callout kind="success">Cloned. It shows up in Source control now.</Callout>}
        <pre className="git-output git-progress">{lines.join('\n') || 'Starting git clone…'}</pre>
      </div>
    </Modal>
  );
}

// ---------- switch branch across repos ----------

const PLAN_LABEL: Record<SwitchPlan, { text: string; tone: string }> = {
  already: { text: 'already on it', tone: 'outline' },
  local: { text: 'switch', tone: 'accent' },
  remote: { text: 'track remote branch', tone: 'info' },
  create: { text: 'create from current', tone: 'warning' },
  skip: { text: 'no such branch — skipped', tone: 'outline' },
  unknown: { text: '', tone: 'outline' },
};

function SwitchBranchDialog({ root, initial, onClose }: { root: string; initial?: string[]; onClose(): void }) {
  const repos = useRepos((s) => s.repos);
  const present = useMemo(() => repos.filter((r) => r.state === 'found'), [repos]);
  const [selected, setSelected] = useState<Set<string>>(() => new Set(initial?.length ? initial : present.map((r) => r.name)));
  const [branches, setBranches] = useState<RepoBranches[] | null>(null);
  const [branch, setBranch] = useState('');
  const [create, setCreate] = useState(true);
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<BulkResult[] | null>(null);
  useEffect(() => {
    gitApi
      .branchesAll(root, present.map((r) => r.name))
      .then(setBranches)
      .catch((e) => {
        setBranches([]);
        toast.error('Could not read branches', errorMessage(e));
      });
  }, [root, present]);
  const byRepo = useMemo(() => new Map((branches ?? []).map((b) => [b.repo, b])), [branches]);
  const suggestions = useMemo(() => branchUnion(branches ?? []), [branches]);
  const valid = /^[^\s~^:?*[\\]+$/.test(branch.trim()) && !branch.trim().startsWith('-') && !branch.includes('..');
  const targets = present.filter((r) => selected.has(r.name));
  const acting = targets.filter((r) => ['local', 'remote', 'create'].includes(switchPlan(byRepo.get(r.name), branch, create)));

  const run = async () => {
    setBusy(true);
    try {
      const res = await gitApi.switchAll(root, acting.map((r) => r.name), branch.trim(), create);
      setResults(res);
      if (res.every((r) => r.ok)) toast.success(`Switched to ${branch.trim()}`, bulkSummary(res));
      await useRepos.getState().refreshOverview(acting.map((r) => r.name)).catch(() => {});
    } catch (e) {
      toast.error('Switch failed', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={onClose}
      size="lg"
      title={
        <span className="row-sm">
          <GitBranch size={16} /> Switch branch across repositories
        </span>
      }
      footer={
        results ? (
          <button className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        ) : (
          <>
            <button className="btn" onClick={onClose}>
              Cancel
            </button>
            <button className="btn btn-primary" disabled={busy || !valid || !acting.length} onClick={run}>
              {busy ? <Spinner /> : <GitBranch />} Switch {acting.length} {acting.length === 1 ? 'repository' : 'repositories'}
            </button>
          </>
        )
      }
    >
      {results ? (
        <ResultsTable results={results} />
      ) : (
        <div className="col">
          <Field label="Branch" hint="An existing local branch is checked out; a branch that only exists on the remote is tracked. Uncommitted changes that would be overwritten make git refuse — nothing is stashed or discarded.">
            <input className="input mono" list="git-switch-branches" autoFocus value={branch} placeholder="feature/checkout-v2" onChange={(e) => setBranch(e.target.value)} />
            <datalist id="git-switch-branches">
              {suggestions.map((b) => (
                <option key={b} value={b} />
              ))}
            </datalist>
          </Field>
          <Checkbox checked={create} onChange={setCreate} label="Create the branch (from the current HEAD) where it doesn't exist" />
          {!present.length ? (
            <Empty title="No repositories on this machine" />
          ) : (
            <div className="git-table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th style={{ width: 28 }}>
                      <Checkbox checked={targets.length === present.length} indeterminate={!!targets.length && targets.length < present.length} onChange={(v) => setSelected(new Set(v ? present.map((r) => r.name) : []))} />
                    </th>
                    <th>Repository</th>
                    <th>Current branch</th>
                    <th>Will</th>
                  </tr>
                </thead>
                <tbody>
                  {present.map((r) => {
                    const b = byRepo.get(r.name);
                    const plan = switchPlan(b, branch, create);
                    const on = selected.has(r.name);
                    return (
                      <tr key={r.name} className={clsx('clickable', !on && 'git-dim')} onClick={() => setSelected((s) => (s.has(r.name) ? new Set([...s].filter((x) => x !== r.name)) : new Set([...s, r.name])))}>
                        <td>
                          <Checkbox checked={on} onChange={() => {}} />
                        </td>
                        <td>
                          <span className="strong">{r.name}</span> {r.group && <span className="badge outline">{r.group}</span>}
                        </td>
                        <td className="mono xs">{branches ? (b?.branches?.current ?? (b?.error ? <span className="subtle">{b.error}</span> : '(detached)')) : <Spinner />}</td>
                        <td>{on && branch.trim() && PLAN_LABEL[plan].text && <span className={clsx('badge', PLAN_LABEL[plan].tone)}>{PLAN_LABEL[plan].text}</span>}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}
    </Modal>
  );
}

// ---------- the repositories manager ----------

interface Adding {
  folder: string;
  s: RepoSuggestion;
  name: string;
  group: string;
  local: boolean;
}

const ORIGIN_HINT: Record<Repo['origin'], string> = {
  manifest: 'Listed in devall.yaml',
  containing: 'The repository the project folder is in (found automatically)',
  discovered: 'Found under the project folder, not listed in devall.yaml yet',
  local: 'A worktree opened on this machine only — never written to devall.yaml',
};

export function RepoBadges({ repo }: { repo: Repo }) {
  return (
    <>
      {repo.state === 'missing' && <span className="badge warning" title={`Not at ${repo.path} on this machine`}>missing</span>}
      {repo.state === 'notARepo' && <span className="badge warning" title="The folder exists but isn't a git repository">not a repo</span>}
      {repo.origin === 'discovered' && <span className="badge info" title={ORIGIN_HINT.discovered}>discovered</span>}
      {repo.origin === 'local' && <span className="badge outline" title={ORIGIN_HINT.local}>worktree</span>}
      {repo.overridden && <span className="badge outline" title="Location set on this machine only (not in devall.yaml)">this machine</span>}
    </>
  );
}

function ReposDialog({ root, onClose }: { root: string; onClose(): void }) {
  const repos = useRepos((s) => s.repos);
  const [adding, setAdding] = useState<Adding | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    void useRepos.getState().load().catch((e) => toast.error('Could not read repositories', errorMessage(e)));
  }, []);
  const groups = useMemo(() => [...new Set(repos.map((r) => r.group).filter((g): g is string => !!g))], [repos]);
  const discovered = repos.filter((r) => r.origin === 'discovered');

  const act = async (label: string, fn: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await fn();
      await refreshAll();
    } catch (e) {
      toast.error(`${label} failed`, errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const pick = async () => {
    const dir = await openDialog({ directory: true, title: 'Choose a repository folder' });
    if (typeof dir !== 'string') return;
    try {
      const s = await reposApi.suggest(root, dir);
      if (s.existing) return toast.info(`Already in the project as ${s.existing}`);
      setAdding({ folder: dir, s, name: s.name, group: '', local: s.farAway || !s.relPath });
    } catch (e) {
      toast.error('Could not read the folder', errorMessage(e));
    }
  };

  const add = (a: Adding) =>
    act('Add repository', async () => {
      const r = await reposApi.add(root, a.folder, { name: a.name.trim() || undefined, group: a.group.trim() || undefined, localOnly: a.local });
      toast.success(`Added ${r.name}`, a.local ? 'Its location is kept on this machine; teammates locate or clone it.' : `devall.yaml: ${r.path}`);
      setAdding(null);
    });

  const menu = (r: Repo): MenuItemDef[] => {
    const items: MenuItemDef[] = [];
    if (r.origin === 'manifest') {
      items.push(
        {
          label: 'Rename…',
          icon: <Pencil />,
          onSelect: async () => {
            const n = await promptDialog({ title: `Rename ${r.name}`, label: 'Name (links use repo:<name>/…)', initial: r.name, validate: (v) => (/^[^/\\:.][^/\\:]*$/.test(v.trim()) ? undefined : 'No slashes, colons or leading dots') });
            if (n && n.trim() !== r.name) void act('Rename', () => reposApi.update(root, r.name, { newName: n.trim() }));
          },
        },
        {
          label: 'Set group…',
          icon: <Tag />,
          onSelect: async () => {
            const g = await promptDialog({ title: `Group of ${r.name}`, label: `e.g. Frontend, Backend${groups.length ? ` (used: ${groups.join(', ')})` : ''}; empty = none`, initial: r.group ?? '' });
            if (g !== null) void act('Set group', () => reposApi.update(root, r.name, { group: g.trim() }));
          },
        },
        { label: r.state === 'missing' ? 'Locate…' : 'Change location on this machine…', icon: <FolderSearch />, onSelect: () => void locateRepo(r) },
      );
      if (r.state === 'missing') items.push({ label: 'Clone…', icon: <Download />, disabled: !r.remote, hint: r.remote ? undefined : 'no remote', onSelect: () => void cloneRepo(r) });
    }
    if (r.origin === 'discovered') items.push({ label: 'Save to devall.yaml', icon: <Save />, onSelect: () => void saveDiscovered([r.name]) });
    if (r.state !== 'missing') items.push({ label: 'Reveal in file manager', icon: <FolderOpen />, onSelect: () => void revealItemInDir(r.path).catch((e) => toast.error('Could not reveal', errorMessage(e))) });
    if (r.origin === 'manifest')
      items.push(
        { separator: true },
        {
          label: 'Remove from project',
          icon: <Trash2 />,
          danger: true,
          onSelect: async () => {
            if (await confirmDialog({ title: `Remove ${r.name} from the project?`, message: 'It is removed from devall.yaml (and this machine’s location). Its files stay where they are.', confirmText: 'Remove', danger: true }))
              void act('Remove', () => reposApi.remove(root, r.name));
          },
        },
      );
    return items;
  };

  return (
    <Modal
      open
      onClose={onClose}
      size="xl"
      title={
        <span className="row-sm">
          <GitBranch size={16} /> Repositories
        </span>
      }
      footer={
        <>
          {discovered.length > 0 && (
            <button className="btn" disabled={busy} onClick={() => void saveDiscovered(discovered.map((r) => r.name))}>
              <Save /> Save {discovered.length} discovered to devall.yaml
            </button>
          )}
          <span className="spacer" />
          <button className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        </>
      }
    >
      <div className="col">
        <p className="muted small">
          A project can span several repositories — say, two frontends and three backends — that share its requests, environments, mocks and notes. They're listed in{' '}
          <code className="code-inline">devall.yaml</code> with paths relative to the project folder (commit it to share them). When a repo lives elsewhere on your machine, “Locate…” stores that location on this
          machine only. Link to files in a repo as <code className="code-inline">repo:&lt;name&gt;/&lt;path&gt;</code>.
        </p>
        {adding ? (
          <div className="card col">
            <div className="row-sm">
              <FolderOpen size={14} className="subtle" />
              <span className="mono xs truncate grow">{adding.folder}</span>
            </div>
            {!adding.s.isGit && <Callout kind="warning">This folder isn't a git repository yet. You can still add it; Source control offers “Initialize repository”.</Callout>}
            <div className="grid-2">
              <Field label="Name" hint="Links use repo:<name>/<path>">
                <input className="input" value={adding.name} onChange={(e) => setAdding({ ...adding, name: e.target.value })} />
              </Field>
              <Field label="Group" hint="e.g. Frontend, Backend (optional)">
                <input className="input" list="git-repo-groups" value={adding.group} onChange={(e) => setAdding({ ...adding, group: e.target.value })} />
                <datalist id="git-repo-groups">
                  {groups.map((g) => (
                    <option key={g} value={g} />
                  ))}
                </datalist>
              </Field>
            </div>
            <Field
              label="Where teammates find it"
              hint={
                adding.local
                  ? `devall.yaml gets ../${adding.folder.split(/[\\/]/).filter(Boolean).pop()} (a guess teammates can locate or clone); this machine remembers the real folder.`
                  : `devall.yaml gets “${adding.s.relPath}”, relative to the project folder.`
              }
            >
              {adding.s.relPath ? (
                <Segmented
                  value={adding.local ? 'local' : 'relative'}
                  onChange={(v) => setAdding({ ...adding, local: v === 'local' })}
                  options={[
                    { value: 'relative', label: `Relative path (${adding.s.relPath})`, title: 'Shared in devall.yaml' },
                    { value: 'local', label: 'This machine only', title: 'Kept outside the repo' },
                  ]}
                />
              ) : (
                <span className="small muted">It's on another drive, so its location is kept on this machine only.</span>
              )}
            </Field>
            {adding.s.farAway && adding.s.relPath && !adding.local && <Callout kind="warning">That folder is far from the project folder; teammates are unlikely to have the same layout.</Callout>}
            <div className="row-sm">
              <span className="spacer" />
              <button className="btn" onClick={() => setAdding(null)}>
                Cancel
              </button>
              <button className="btn btn-primary" disabled={busy || !adding.name.trim()} onClick={() => add(adding)}>
                <Plus /> Add repository
              </button>
            </div>
          </div>
        ) : (
          <div className="row-sm">
            <button className="btn btn-sm" onClick={pick} disabled={busy}>
              <Plus /> Add repository…
            </button>
            {busy && <Spinner />}
          </div>
        )}
        {repos.length === 0 ? (
          <Empty icon={<GitBranch />} title="No repositories yet">
            The project folder isn't in a git repository and lists none. Add the repositories this project's requests and environments are for.
          </Empty>
        ) : (
          <div className="git-table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Group</th>
                  <th>Location</th>
                  <th>State</th>
                  <th style={{ width: 40 }} />
                </tr>
              </thead>
              <tbody>
                {groupRepos(repos).flatMap((g) =>
                  g.repos.map((r) => (
                    <tr key={r.name}>
                      <td>
                        <span className="strong">{r.name}</span>
                      </td>
                      <td className="small">{r.group ?? <span className="subtle">—</span>}</td>
                      <td className="git-loc">
                        <div className="mono xs truncate" title={r.path}>
                          {r.relPath ?? (r.projectPrefix ? `contains the project (${r.projectPrefix})` : 'the project folder')}
                        </div>
                        {(r.overridden || r.origin !== 'manifest') && <div className="xs subtle truncate" title={r.path}>{r.path}</div>}
                      </td>
                      <td>
                        <span className="row-sm wrap" title={ORIGIN_HINT[r.origin]}>
                          {r.state === 'found' && r.origin !== 'discovered' && <span className="badge success">{r.origin === 'containing' ? 'project repo' : 'found'}</span>}
                          <RepoBadges repo={r} />
                        </span>
                      </td>
                      <td>
                        {menu(r).length > 0 && (
                          <Menu
                            align="right"
                            items={() => menu(r)}
                            trigger={(p) => (
                              <button className="icon-btn sm" title="Actions" {...p}>
                                <MoreHorizontal />
                              </button>
                            )}
                          />
                        )}
                      </td>
                    </tr>
                  )),
                )}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </Modal>
  );
}
