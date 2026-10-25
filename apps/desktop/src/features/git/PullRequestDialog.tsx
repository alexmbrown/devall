import { useEffect, useState } from 'react';
import { create } from 'zustand';
import { ExternalLink, GitPullRequestCreate, KeyRound, LogOut, Terminal } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, CopyButton, Field, Segmented, Spinner, copyText } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type Comparison, type CreatedPullRequest, type ForgeInfo, type ForgeKind } from '@/lib/api/git';
import { hostCompareUrl, markdownSummary } from './model';
import { defaultTemplate, nouns, prBody, prTitle, splitNames } from './pr';

type Open = { root: string; repo: string | null; comparison: Comparison } | null;
type Connect = { host: string; kind: ForgeKind; onDone(): void } | null;

const usePrDialog = create<{ open: Open; connect: Connect; show(o: Open): void; showConnect(c: Connect): void }>((set) => ({
  open: null,
  connect: null,
  show: (open) => set({ open }),
  showConnect: (connect) => set({ connect }),
}));

/** Opens "Create pull/merge request" for a comparison. */
export const openPullRequest = (o: NonNullable<Open>) => usePrDialog.getState().show(o);

export function PullRequestHost() {
  const open = usePrDialog((s) => s.open);
  const connect = usePrDialog((s) => s.connect);
  return (
    <>
      {open && <PullRequestDialog {...open} onClose={() => usePrDialog.getState().show(null)} />}
      {connect && <ConnectDialog {...connect} onClose={() => usePrDialog.getState().showConnect(null)} />}
    </>
  );
}

/**
 * Create a pull request (GitHub, GitHub Enterprise) or merge request (GitLab, self-managed) from
 * Compare & review: title and description prefilled (the repository's template, then commits and
 * files), target/source, draft, reviewers, labels. Pushes the source branch first when it isn't
 * on the remote (asked). Uses a token from the OS keychain, or the `gh` / `glab` CLI.
 */
function PullRequestDialog({ root, repo, comparison: c, onClose }: NonNullable<Open> & { onClose(): void }) {
  const [info, setInfo] = useState<ForgeInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [title, setTitle] = useState(() => prTitle(c, c.headRemoteBranch));
  const [body, setBody] = useState('');
  const [template, setTemplate] = useState<string>('');
  const [base, setBase] = useState(c.baseRemoteBranch ?? c.base.name);
  const [head, setHead] = useState(c.headRemoteBranch ?? c.head.name);
  const [draft, setDraft] = useState(false);
  const [reviewers, setReviewers] = useState('');
  const [labels, setLabels] = useState('');
  const [push, setPush] = useState(!c.headPushed);
  const [via, setVia] = useState<'token' | 'cli'>('token');
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState<CreatedPullRequest | null>(null);
  const [v, setV] = useState(0);

  useEffect(() => {
    gitApi
      .forgeInfo(root, repo, c.baseRemote)
      .then((i) => {
        setInfo(i);
        setVia(i.connected || !i.cli?.installed ? 'token' : 'cli');
        const t = defaultTemplate(i.templates);
        setTemplate((cur) => cur || t?.path || '');
        setBody((cur) => cur || prBody(c, t?.content ?? null));
      })
      .catch((e) => setError(errorMessage(e)));
  }, [root, repo, c, v]);

  const n = nouns(info?.forge?.kind);
  const remote = info?.remoteName ?? c.baseRemote ?? 'origin';
  const compareUrl = hostCompareUrl(c.remoteUrl, base, head);
  const fallback = async () => {
    await copyText(markdownSummary(c), 'Summary copied as Markdown — paste it into the merge request');
    if (compareUrl) await openUrl(compareUrl).catch((e) => toast.error('Could not open the browser', errorMessage(e)));
  };

  const create = async () => {
    if (!info?.forge) return;
    if (push && !(await confirmDialog({ title: `Push ${c.head.name} to ${remote} first?`, message: `Runs git push -u ${remote} ${c.head.name} so ${info.forge.host} has the branch, then opens the ${n.noun}.`, confirmText: 'Push and continue' }))) return;
    setBusy(true);
    try {
      const res = await gitApi.forgeCreate(root, repo, remote, { title: title.trim(), body, base, head, draft, reviewers: splitNames(reviewers), labels: splitNames(labels) }, push, via === 'cli');
      setCreated(res);
      for (const w of res.warnings) toast.info('Created, with a problem', w);
    } catch (e) {
      toast.error(`Creating the ${n.noun} failed`, errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  if (created)
    return (
      <Modal open onClose={onClose} title={`${n.short} #${created.number} created`} footer={<button className="btn btn-primary" onClick={onClose}>Done</button>}>
        <div className="col">
          <Callout kind="success">
            {n.noun[0].toUpperCase() + n.noun.slice(1)} #{created.number} is open{created.viaCli ? ` (via ${info?.cli?.program})` : ''}.
          </Callout>
          <div className="row-sm">
            <span className="mono xs truncate grow">{created.url}</span>
            <CopyButton text={created.url} />
            <button className="btn btn-sm btn-primary" onClick={() => void openUrl(created.url).catch((e) => toast.error('Could not open the browser', errorMessage(e)))}>
              <ExternalLink /> Open in browser
            </button>
          </div>
          {created.warnings.map((w) => (
            <Callout key={w} kind="warning">
              {w}
            </Callout>
          ))}
        </div>
      </Modal>
    );

  const canToken = !!info?.connected;
  const canCli = !!info?.cli?.installed;
  const ready = !!info?.forge && (via === 'token' ? canToken : canCli);

  return (
    <Modal
      open
      onClose={busy ? () => {} : onClose}
      size="xl"
      title={
        <span className="row-sm">
          <GitPullRequestCreate size={16} /> Create {n.noun}
          {info?.forge && <span className="subtle small">· {info.forge.host}/{info.forge.path}</span>}
        </span>
      }
      footer={
        <>
          <button className="btn btn-ghost" onClick={() => void fallback()} title="Copy the Markdown summary and open the host's compare page instead">
            <ExternalLink /> Open compare page instead
          </button>
          <span className="spacer" />
          <button className="btn" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy || !ready || !title.trim() || !base || !head || base === head} onClick={() => void create()}>
            {busy ? <Spinner /> : <GitPullRequestCreate />} Create {n.short}
          </button>
        </>
      }
    >
      {error ? (
        <Callout kind="danger">{error}</Callout>
      ) : !info ? (
        <div className="row-sm subtle small">
          <Spinner /> Looking at the remote…
        </div>
      ) : !info.forge ? (
        <div className="col">
          <Callout kind="info">
            {info.remoteUrl ? <span className="mono xs">{info.remoteUrl}</span> : 'This repository has no remote'} isn't a GitHub or GitLab host /dev/all recognises. For GitHub Enterprise or self-managed GitLab under another name, connect the host first.
          </Callout>
          <div className="row-sm">
            {info.remoteUrl && (
              <>
                <button className="btn btn-sm" onClick={() => usePrDialog.getState().showConnect({ host: hostOf(info.remoteUrl!) ?? '', kind: 'gitlab', onDone: () => setV((x) => x + 1) })}>
                  <KeyRound /> Connect as GitLab…
                </button>
                <button className="btn btn-sm" onClick={() => usePrDialog.getState().showConnect({ host: hostOf(info.remoteUrl!) ?? '', kind: 'github', onDone: () => setV((x) => x + 1) })}>
                  <KeyRound /> Connect as GitHub Enterprise…
                </button>
              </>
            )}
          </div>
        </div>
      ) : (
        <div className="col git-pr-form">
          <div className="row-sm wrap">
            {canToken ? (
              <span className="small">
                <KeyRound size={13} className="git-ok" /> {info.forge.host} connected{info.account?.user ? ` as @${info.account.user}` : ''} (token in the OS keychain{info.keychain ? '' : ' — unavailable: kept until the app quits'})
              </span>
            ) : (
              <span className="small subtle">{info.forge.host} isn't connected.</span>
            )}
            <span className="spacer" />
            {canToken ? (
              <button
                className="btn btn-sm btn-ghost"
                onClick={async () => {
                  if (await confirmDialog({ title: `Disconnect ${info.forge!.host}?`, message: 'Removes its token from the OS keychain.', confirmText: 'Disconnect' })) {
                    await gitApi.forgeDisconnect(info.forge!.host).catch((e) => toast.error('Disconnect failed', errorMessage(e)));
                    setV((x) => x + 1);
                  }
                }}
              >
                <LogOut /> Disconnect
              </button>
            ) : (
              <button className="btn btn-sm" onClick={() => usePrDialog.getState().showConnect({ host: info.forge!.host, kind: info.forge!.kind, onDone: () => setV((x) => x + 1) })}>
                <KeyRound /> Connect {n.host}…
              </button>
            )}
          </div>
          {(canToken || canCli) && canToken !== canCli ? null : canToken && canCli ? (
            <Segmented
              value={via}
              onChange={setVia}
              options={[
                { value: 'token', label: 'Use the token' },
                { value: 'cli', label: `Use ${info.cli?.program}` },
              ]}
            />
          ) : null}
          {!canToken && canCli && (
            <Callout kind="info">
              <span className="row-sm">
                <Terminal size={13} /> No token — {info.cli?.program} (installed) creates it with its own sign-in.
              </span>
            </Callout>
          )}
          {!canToken && !canCli && <Callout kind="warning">Connect {info.forge.host} (a personal access token, kept in the OS keychain) — or install and sign in to {info.forge.kind === 'github' ? 'gh' : 'glab'}.</Callout>}
          <div className="git-pr-branches">
            <Field label="Into (target)">
              <input className="input mono" value={base} onChange={(e) => setBase(e.target.value.trim())} />
            </Field>
            <Field label="From (source)">
              <input className="input mono" value={head} onChange={(e) => setHead(e.target.value.trim())} />
            </Field>
          </div>
          <Field label="Title">
            <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus />
          </Field>
          <Field
            label={
              <span className="row-sm">
                Description
                {info.templates.length > 0 && (
                  <select
                    className="select git-pr-template"
                    value={template}
                    onChange={(e) => {
                      setTemplate(e.target.value);
                      const t = info.templates.find((x) => x.path === e.target.value);
                      setBody(prBody(c, t?.content ?? null));
                    }}
                  >
                    <option value="">No template</option>
                    {info.templates.map((t) => (
                      <option key={t.path} value={t.path}>
                        Template: {t.name}
                      </option>
                    ))}
                  </select>
                )}
              </span>
            }
          >
            <textarea className="textarea mono git-pr-body" rows={12} value={body} onChange={(e) => setBody(e.target.value)} />
          </Field>
          <div className="git-pr-branches">
            <Field label="Reviewers" hint={info.forge.kind === 'github' ? 'User names, or org/team' : 'User names'}>
              <input className="input" value={reviewers} onChange={(e) => setReviewers(e.target.value)} placeholder="alice, bob" />
            </Field>
            <Field label="Labels">
              <input className="input" value={labels} onChange={(e) => setLabels(e.target.value)} placeholder="feature, needs-review" />
            </Field>
          </div>
          <div className="row-sm wrap">
            <Checkbox checked={draft} onChange={setDraft} label={<span className="small">Draft</span>} />
            {!c.headPushed && <Checkbox checked={push} onChange={setPush} label={<span className="small">Push {c.head.name} to {remote} first (sets its upstream)</span>} />}
          </div>
        </div>
      )}
    </Modal>
  );
}

function hostOf(url: string): string | null {
  const m = /^[a-z][a-z0-9+.-]*:\/\/(?:[^@/]+@)?([^/]+)/i.exec(url) ?? /^(?:[^@/]+@)?([^/:]+):/.exec(url);
  return m ? m[1].toLowerCase() : null;
}

/** "Connect GitHub/GitLab": a personal access token, checked with the host, kept in the OS keychain. */
function ConnectDialog({ host: initialHost, kind: initialKind, onDone, onClose }: NonNullable<Connect> & { onClose(): void }) {
  const [kind, setKind] = useState<ForgeKind>(initialKind);
  const [host, setHost] = useState(initialHost || (initialKind === 'github' ? 'github.com' : 'gitlab.com'));
  const [token, setToken] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const bare = host.split(':')[0];
  const tokenUrl = kind === 'github' ? `https://${host}/settings/tokens/new?scopes=repo&description=devall` : `https://${host}/-/user_settings/personal_access_tokens?name=devall&scopes=api`;
  const scopes = kind === 'github' ? 'Classic token: repo (or public_repo for public repositories only). Fine-grained: Pull requests read & write, Contents read.' : 'Scope: api';
  const connect = async () => {
    setBusy(true);
    setError(null);
    try {
      const a = await gitApi.forgeConnect(host, kind, token);
      setToken('');
      toast.success(`Connected ${a.host}`, a.user ? `as @${a.user}` : undefined);
      onDone();
      onClose();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      open
      onClose={busy ? () => {} : onClose}
      title={`Connect ${kind === 'github' ? 'GitHub' : 'GitLab'}`}
      footer={
        <>
          <button className="btn" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy || !token.trim() || !host.trim()} onClick={() => void connect()}>
            {busy ? <Spinner /> : <KeyRound />} Connect
          </button>
        </>
      }
    >
      <div className="col">
        <Segmented
          value={kind}
          onChange={(k) => {
            setKind(k);
            if (host === 'github.com' || host === 'gitlab.com') setHost(k === 'github' ? 'github.com' : 'gitlab.com');
          }}
          options={[
            { value: 'github', label: 'GitHub / Enterprise' },
            { value: 'gitlab', label: 'GitLab / self-managed' },
          ]}
        />
        <Field label="Host">
          <input className="input mono" value={host} onChange={(e) => setHost(e.target.value.trim())} />
        </Field>
        <Field label="Personal access token" hint={scopes}>
          <input className="input mono" type="password" autoComplete="off" spellCheck={false} value={token} onChange={(e) => setToken(e.target.value)} />
        </Field>
        <button className="btn btn-sm btn-ghost" style={{ alignSelf: 'flex-start' }} disabled={!bare} onClick={() => void openUrl(tokenUrl).catch((e) => toast.error('Could not open the browser', errorMessage(e)))}>
          <ExternalLink /> Create a token on {host || 'the host'}
        </button>
        <span className="xs subtle">The token is checked with {host || 'the host'} once, then stored only in your OS keychain — never in devall.yaml, logs or the app's settings. Nothing is sent until you create a {kind === 'github' ? 'pull' : 'merge'} request.</span>
        {error && <Callout kind="danger">{error}</Callout>}
      </div>
    </Modal>
  );
}
