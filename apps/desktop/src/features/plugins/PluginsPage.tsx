import { useCallback, useEffect, useState } from 'react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { FileArchive, FolderOpen, KeyRound, Puzzle, RefreshCw, ShieldAlert, ShieldCheck, Trash2 } from 'lucide-react';
import { Callout, Checkbox, Spinner, Switch } from '@/components/ui/misc';
import { Modal } from '@/components/ui/Modal';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { pluginsApi, statusLabel, usePluginCatalog, type InstallPreview, type PluginInfo, type PluginManifest, type PluginScope } from '@/lib/api/plugins';
import { useWorkspace } from '@/store/workspace';
import { contributionSummary, statusTone } from './model';

/** What the review dialog shows: a package to install, or a plugin to (re)approve. */
type Review =
  | { kind: 'install'; source: string; preview: InstallPreview }
  | { kind: 'trust'; plugin: PluginInfo };

/**
 * WebAssembly plugins: installed ones (`<app data>/plugins`) and the open project's
 * (`<project>/plugins`, which never run until trusted here). Install, trust, enable, grant
 * secrets, toggle hooks, uninstall.
 */
export default function PluginsPage() {
  const root = useWorkspace((s) => s.project?.root ?? null);
  const [list, setList] = useState<PluginInfo[] | null>(null);
  const [review, setReview] = useState<Review | null>(null);
  const [secretFor, setSecretFor] = useState<{ plugin: string; name: string } | null>(null);

  const reload = useCallback(async () => {
    try {
      setList(await pluginsApi.list(root));
    } catch (e) {
      toast.error('Could not list plugins', errorMessage(e));
      setList([]);
    }
    void usePluginCatalog.getState().refresh(root);
  }, [root]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const pick = async (zip: boolean) => {
    const picked = await openDialog(zip ? { multiple: false, filters: [{ name: 'Plugin package', extensions: ['zip'] }] } : { directory: true, multiple: false });
    if (typeof picked !== 'string') return;
    try {
      setReview({ kind: 'install', source: picked, preview: await pluginsApi.preview(picked) });
    } catch (e) {
      toast.error('Not a valid plugin', errorMessage(e));
    }
  };

  const act = async (what: string, f: () => Promise<unknown>) => {
    try {
      await f();
      await reload();
    } catch (e) {
      toast.error(what, errorMessage(e));
    }
  };

  const installed = list?.filter((p) => p.scope === 'user') ?? [];
  const project = list?.filter((p) => p.scope === 'project') ?? [];

  return (
    <div className="page">
      <div className="page-header">
        <h1>Plugins</h1>
        <div className="spacer" />
        <button className="btn btn-sm" onClick={() => void pick(false)} title="Install a plugin folder (plugin.yaml + plugin.wasm)">
          <FolderOpen size={14} /> Install from folder…
        </button>
        <button className="btn btn-sm" onClick={() => void pick(true)}>
          <FileArchive size={14} /> Install from .zip…
        </button>
        <button
          className="icon-btn"
          title="Open the plugins folder"
          onClick={() => void pluginsApi.dir().then((d) => revealItemInDir(d)).catch((e) => toast.error('Could not open the folder', errorMessage(e)))}
        >
          <FolderOpen size={14} />
        </button>
        <button className="icon-btn" title="Refresh" onClick={() => void reload()}>
          <RefreshCw size={14} />
        </button>
      </div>
      <div className="page-body col-lg" style={{ maxWidth: 920 }}>
        <p className="muted small">
          Plugins are sandboxed WebAssembly modules that add utilities, request hooks, import formats, code generators and auth providers. They get no file system,
          network or secrets unless their manifest asks and you approve it. See <span className="code-inline">docs/PLUGINS.md</span> to write one.
        </p>
        {list === null ? (
          <div className="empty">
            <Spinner />
          </div>
        ) : (
          <>
            <Section title="Installed" empty="No plugins installed yet. Install a plugin folder or .zip above.">
              {installed.map((p) => (
                <PluginCard key={`u:${p.id}`} p={p} root={root} act={act} onReview={() => setReview({ kind: 'trust', plugin: p })} onSecret={(name) => setSecretFor({ plugin: p.id, name })} />
              ))}
            </Section>
            {root && (
              <Section title="This project" empty="This project has no plugins (they live in plugins/<id>/ next to devall.yaml).">
                {project.map((p) => (
                  <PluginCard key={`p:${p.id}`} p={p} root={root} act={act} onReview={() => setReview({ kind: 'trust', plugin: p })} onSecret={(name) => setSecretFor({ plugin: p.id, name })} />
                ))}
              </Section>
            )}
          </>
        )}
      </div>
      {review && <ReviewDialog review={review} root={root} onClose={() => setReview(null)} onDone={() => void reload()} />}
      {secretFor && <SecretDialog plugin={secretFor.plugin} name={secretFor.name} onClose={() => setSecretFor(null)} onDone={() => void reload()} />}
    </div>
  );
}

function Section({ title, empty, children }: { title: string; empty: string; children: React.ReactNode[] }) {
  return (
    <div className="col">
      <div className="section-title">{title}</div>
      {children.length ? children : <p className="subtle small">{empty}</p>}
    </div>
  );
}

function PluginCard({
  p,
  root,
  act,
  onReview,
  onSecret,
}: {
  p: PluginInfo;
  root: string | null;
  act(what: string, f: () => Promise<unknown>): Promise<void>;
  onReview(): void;
  onSecret(name: string): void;
}) {
  const m = p.manifest;
  const tone = statusTone(p.status);
  const scopeRoot = p.scope === 'project' ? root : null;
  const hooks = m?.contributes.hooks ?? [];
  const approved = p.status === 'ready' || p.status === 'disabled';
  return (
    <div className="card col" data-testid={`plugin-${p.id}`}>
      <div className="row">
        <Puzzle size={16} className="muted" />
        <strong>{m?.name ?? p.id}</strong>
        {m && <span className="subtle xs mono">{m.version}</span>}
        <span className="subtle xs mono">{p.id}</span>
        <span className={`badge ${tone}`}>{statusLabel(p.status)}</span>
        <div className="spacer" />
        {approved && (
          <Switch checked={p.enabled} onChange={(v) => void act(v ? 'Could not enable' : 'Could not disable', () => pluginsApi.setEnabled(scopeRoot, p.scope, p.id, v))} label="Enabled" />
        )}
        {(p.status === 'untrusted' || p.status === 'changed') && (
          <button className="btn btn-sm btn-primary" onClick={onReview}>
            <ShieldCheck size={14} /> {p.status === 'changed' ? 'Review changes…' : 'Review and trust…'}
          </button>
        )}
        <button className="icon-btn" title="Reveal folder" onClick={() => void revealItemInDir(p.dir).catch((e) => toast.error('Could not reveal', errorMessage(e)))}>
          <FolderOpen size={14} />
        </button>
        {approved && p.scope === 'project' && (
          <button
            className="icon-btn"
            title="Revoke trust"
            onClick={() => void act('Could not revoke', () => pluginsApi.revoke(scopeRoot, p.scope, p.id))}
          >
            <ShieldAlert size={14} />
          </button>
        )}
        {p.scope === 'user' && (
          <button
            className="icon-btn danger"
            title="Uninstall"
            onClick={async () => {
              if (await confirmDialog({ title: `Uninstall ${m?.name ?? p.id}?`, message: 'The plugin folder moves to the OS trash.', confirmText: 'Uninstall', danger: true }))
                void act('Could not uninstall', () => pluginsApi.uninstall(p.id));
            }}
          >
            <Trash2 size={14} />
          </button>
        )}
      </div>
      {m?.description && <p className="small">{m.description}</p>}
      {p.error && <Callout kind={p.status === 'shadowed' ? 'warning' : 'danger'}>{p.error}</Callout>}
      {p.status === 'changed' && <Callout kind="warning">The module or its capabilities changed since you approved it. It won’t run until you review it again.</Callout>}
      {p.status === 'untrusted' && p.scope === 'project' && (
        <Callout kind="info">This plugin came with the project. It won’t run until you review it and trust it on this machine.</Callout>
      )}
      {m && <div className="subtle xs">{contributionSummary(m)}</div>}
      <dl className="kv">
        <dt>Capabilities</dt>
        <dd>{p.capabilities.length ? p.capabilities.join(' · ') : 'None (pure computation)'}</dd>
        {p.wasmSha256 && (
          <>
            <dt>SHA-256</dt>
            <dd className="mono xs break">{p.wasmSha256}</dd>
          </>
        )}
      </dl>
      {approved && p.secrets.length > 0 && (
        <div className="col-sm">
          <div className="label">Secrets</div>
          {p.secrets.map((s) => (
            <div key={s.name} className="row">
              <Checkbox
                checked={s.granted}
                label={<span className="mono xs">{s.name}</span>}
                onChange={(v) =>
                  void act('Could not change the grant', () =>
                    pluginsApi.setGrants(scopeRoot, p.scope, p.id, p.secrets.filter((x) => (x.name === s.name ? v : x.granted)).map((x) => x.name)),
                  )
                }
              />
              <span className={`badge ${s.set ? 'success' : 'outline'}`}>{s.set ? 'value set' : 'not set'}</span>
              <button className="btn btn-sm btn-ghost" onClick={() => onSecret(s.name)}>
                <KeyRound size={13} /> {s.set ? 'Change…' : 'Set…'}
              </button>
            </div>
          ))}
        </div>
      )}
      {approved && hooks.length > 0 && (
        <div className="col-sm">
          <div className="label">Request hooks</div>
          {hooks.map((h) => (
            <Switch
              key={h.id}
              checked={!p.disabledHooks.includes(h.id)}
              onChange={(v) => void act('Could not toggle the hook', () => pluginsApi.setHookEnabled(scopeRoot, p.scope, p.id, h.id, v))}
              label={
                <>
                  {h.name} <span className="subtle xs">({h.events.join(', ')}{h.hosts?.length ? ` · ${h.hosts.join(', ')}` : ' · every request'})</span>
                </>
              }
            />
          ))}
        </div>
      )}
    </div>
  );
}

function CapabilityList({ manifest, caps, grants, setGrants }: { manifest: PluginManifest; caps: string[]; grants: string[]; setGrants(g: string[]): void }) {
  const secrets = manifest.capabilities['secrets.read'] ?? [];
  return (
    <div className="col">
      <div className="label">This plugin may</div>
      {caps.length === 0 ? (
        <p className="small">Only compute: no network, files or secrets.</p>
      ) : (
        <ul className="small" style={{ margin: 0, paddingLeft: 18 }}>
          {caps.map((c) => (
            <li key={c}>{c}</li>
          ))}
        </ul>
      )}
      {secrets.length > 0 && (
        <div className="col-sm">
          <div className="subtle xs">Secrets it may read (values you store for it in the OS keychain):</div>
          {secrets.map((s) => (
            <Checkbox key={s} checked={grants.includes(s)} onChange={(v) => setGrants(v ? [...grants, s] : grants.filter((g) => g !== s))} label={<span className="mono xs">{s}</span>} />
          ))}
        </div>
      )}
      <div className="subtle xs">{contributionSummary(manifest)}</div>
    </div>
  );
}

function ReviewDialog({ review, root, onClose, onDone }: { review: Review; root: string | null; onClose(): void; onDone(): void }) {
  const manifest = review.kind === 'install' ? review.preview.manifest : review.plugin.manifest;
  const caps = review.kind === 'install' ? review.preview.capabilities : review.plugin.capabilities;
  const sha = review.kind === 'install' ? review.preview.wasmSha256 : (review.plugin.wasmSha256 ?? '');
  const [grants, setGrants] = useState<string[]>(manifest?.capabilities['secrets.read'] ?? []);
  const [busy, setBusy] = useState(false);
  if (!manifest) return null;
  const go = async () => {
    setBusy(true);
    try {
      if (review.kind === 'install') {
        await pluginsApi.install(review.source, sha, grants);
        toast.success(`Installed ${manifest.name}`);
      } else {
        const scope: PluginScope = review.plugin.scope;
        await pluginsApi.trust(scope === 'project' ? root : null, scope, review.plugin.id, sha, grants);
        toast.success(`Trusted ${manifest.name}`);
      }
      onDone();
      onClose();
    } catch (e) {
      toast.error(review.kind === 'install' ? 'Could not install' : 'Could not trust', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      open
      onClose={onClose}
      title={review.kind === 'install' ? `Install ${manifest.name}?` : `Trust ${manifest.name}?`}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void go()}>
            {busy && <Spinner />} {review.kind === 'install' ? 'Install' : 'Trust and run'}
          </button>
        </>
      }
    >
      <div className="col-lg">
        <div className="row">
          <strong>{manifest.name}</strong>
          <span className="subtle xs mono">
            {manifest.id} {manifest.version}
          </span>
          {manifest.author && <span className="subtle xs">by {manifest.author}</span>}
        </div>
        {manifest.description && <p className="small">{manifest.description}</p>}
        {review.kind === 'install' && review.preview.replaces && <Callout kind="info">Replaces the installed version {review.preview.replaces}.</Callout>}
        {review.kind === 'trust' && review.plugin.scope === 'project' && (
          <Callout kind="warning">This code came with the project repository. Trust it only if you trust where the project came from; a change to it will ask again.</Callout>
        )}
        <CapabilityList manifest={manifest} caps={caps} grants={grants} setGrants={setGrants} />
        <dl className="kv">
          <dt>SHA-256</dt>
          <dd className="mono xs break">{sha}</dd>
          {review.kind === 'install' && (
            <>
              <dt>Source</dt>
              <dd className="mono xs break">{review.source}</dd>
            </>
          )}
        </dl>
      </div>
    </Modal>
  );
}

function SecretDialog({ plugin, name, onClose, onDone }: { plugin: string; name: string; onClose(): void; onDone(): void }) {
  const [value, setValue] = useState('');
  const save = async (v: string | null) => {
    try {
      await pluginsApi.setSecret(plugin, name, v);
      toast.success(v === null ? `Removed ${name}` : `Saved ${name} to the keychain`);
      onDone();
      onClose();
    } catch (e) {
      toast.error('Could not save the secret', errorMessage(e));
    }
  };
  return (
    <Modal
      open
      onClose={onClose}
      title={`Secret ${name} for ${plugin}`}
      footer={
        <>
          <button className="btn btn-danger-ghost" onClick={() => void save(null)}>
            Remove
          </button>
          <div className="spacer" />
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!value} onClick={() => void save(value)}>
            Save
          </button>
        </>
      }
    >
      <div className="col">
        <input className="input mono" type="password" autoFocus value={value} onChange={(e) => setValue(e.target.value)} placeholder="Value (stored in the OS keychain, never in files)" />
        <p className="subtle xs">Only this plugin can read it, and only while the grant is ticked.</p>
      </div>
    </Modal>
  );
}

