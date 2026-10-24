import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { Download, KeyRound, Link2, RefreshCw } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, Empty, Field, SearchInput, Segmented, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { infraApi, type GcpSecret, type GcpSecretVersion, type GcpTarget, type SecretImportMode } from '@/lib/api/infra';
import { useNav } from '@/store/nav';
import { useVarsVersion } from '@/store/variables';
import { useWorkspace } from '@/store/workspace';
import { envSecretName, gcpReference } from './secrets';
import { SidePane } from '@/components/layout/SidePane';

/**
 * Secret Manager (real projects, gcloud login): secrets and versions, metadata only. "Import into
 * environment…" stores values in the OS keychain under an environment (never in files) or adds
 * `gcp-sm://` references to the environment's `secretSources`. Nothing is written to Google Cloud.
 */
export function GcpSecretsPanel({ target }: { target: GcpTarget }) {
  const [secrets, setSecrets] = useState<GcpSecret[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [q, setQ] = useState('');
  const [selected, setSelected] = useState<string | null>(null);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [versions, setVersions] = useState<Record<string, GcpSecretVersion[] | string>>({});
  const [importing, setImporting] = useState<string[] | null>(null);

  const load = () => {
    setSecrets(null);
    setError(null);
    infraApi.gcp.secrets(target).then(setSecrets, (e) => setError(errorMessage(e)));
  };
  useEffect(load, [target]); // eslint-disable-line react-hooks/exhaustive-deps

  const loadVersions = (secret: string) => {
    if (versions[secret]) return;
    infraApi.gcp.secretVersions(target, secret).then(
      (v) => setVersions((m) => ({ ...m, [secret]: v })),
      (e) => setVersions((m) => ({ ...m, [secret]: errorMessage(e) })),
    );
  };
  useEffect(() => {
    if (selected) loadVersions(selected);
  }, [selected]); // eslint-disable-line react-hooks/exhaustive-deps

  const shown = useMemo(() => (secrets ?? []).filter((s) => !q || s.name.toLowerCase().includes(q.toLowerCase())), [secrets, q]);
  const sel = secrets?.find((s) => s.name === selected);
  const selVersions = selected ? versions[selected] : undefined;

  if (error)
    return (
      <div className="p-5">
        <Callout kind="danger">
          <div className="break pre-line">{error}</div>
          <button className="btn btn-sm mt-2" onClick={load}>
            <RefreshCw /> Retry
          </button>
        </Callout>
      </div>
    );

  return (
    <div className="split">
      <SidePane className="gcp-side">
        <div className="toolbar">
          <SearchInput value={q} onChange={setQ} placeholder="Filter secrets…" className="grow" />
          <button className="icon-btn sm" onClick={load} title="Reload" aria-label="Reload secrets">
            <RefreshCw />
          </button>
        </div>
        <div className="toolbar">
          <Checkbox
            checked={shown.length > 0 && shown.every((s) => checked.has(s.name))}
            indeterminate={checked.size > 0 && !shown.every((s) => checked.has(s.name))}
            onChange={(on) => setChecked(on ? new Set(shown.map((s) => s.name)) : new Set())}
            label={<span className="xs subtle">{checked.size ? `${checked.size} selected` : 'Select'}</span>}
          />
          <span className="grow" />
          <button className="btn btn-sm" disabled={checked.size === 0} onClick={() => setImporting([...checked])} title="Store these secrets in an environment">
            <Download /> Import…
          </button>
        </div>
        <div className="list">
          {!secrets ? (
            <div className="empty">
              <Spinner />
            </div>
          ) : shown.length === 0 ? (
            <div className="p-4 xs subtle">{secrets.length ? 'No matches.' : 'No secrets in this project.'}</div>
          ) : (
            shown.map((s) => (
              <div key={s.name} className={clsx('list-item', selected === s.name && 'on')} onClick={() => setSelected(s.name)}>
                <Checkbox
                  checked={checked.has(s.name)}
                  onChange={(on) =>
                    setChecked((c) => {
                      const n = new Set(c);
                      if (on) n.add(s.name);
                      else n.delete(s.name);
                      return n;
                    })
                  }
                />
                <KeyRound size={13} className="subtle" />
                <span className="grow truncate mono small">{s.name}</span>
              </div>
            ))
          )}
        </div>
      </SidePane>
      <div className="pane fill">
        {!sel ? (
          <Empty icon={<KeyRound />} title="Pick a secret">
            Metadata only — values are read just when you import them, straight into your OS keychain. Nothing is written to Google Cloud.
          </Empty>
        ) : (
          <div className="col p-5" style={{ overflow: 'auto' }}>
            <div className="row-sm">
              <KeyRound size={16} />
              <span className="strong mono">{sel.name}</span>
              <span className="grow" />
              <button className="btn btn-sm btn-primary" onClick={() => setImporting([sel.name])}>
                <Download /> Import into environment…
              </button>
            </div>
            <dl className="kv">
              <dt>Resource</dt>
              <dd className="mono xs break">{sel.fullName}</dd>
              <dt>Created</dt>
              <dd>{sel.createTime ?? '—'}</dd>
              <dt>Replication</dt>
              <dd>{sel.replication ?? '—'}</dd>
              {Object.keys(sel.labels).length > 0 && (
                <>
                  <dt>Labels</dt>
                  <dd className="mono xs">
                    {Object.entries(sel.labels)
                      .map(([k, v]) => `${k}=${v}`)
                      .join(', ')}
                  </dd>
                </>
              )}
              <dt>Reference</dt>
              <dd className="mono xs">{gcpReference(target.project, sel.name)}</dd>
            </dl>
            <div className="strong small">Versions</div>
            {selVersions === undefined ? (
              <Spinner />
            ) : typeof selVersions === 'string' ? (
              <Callout kind="danger">{selVersions}</Callout>
            ) : selVersions.length === 0 ? (
              <span className="xs subtle">No versions.</span>
            ) : (
              <table className="table">
                <thead>
                  <tr>
                    <th>Version</th>
                    <th>State</th>
                    <th>Created</th>
                  </tr>
                </thead>
                <tbody>
                  {selVersions.map((v) => (
                    <tr key={v.name}>
                      <td className="mono">{v.name}</td>
                      <td>
                        <span className={clsx('badge', v.state === 'ENABLED' ? 'success' : v.state === 'DESTROYED' ? 'danger' : 'warning')}>{v.state.toLowerCase()}</span>
                      </td>
                      <td className="small">{v.createTime ?? '—'}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        )}
      </div>
      {importing && (
        <ImportDialog
          target={target}
          secrets={importing}
          versions={versions}
          onLoadVersions={loadVersions}
          onClose={() => setImporting(null)}
          onDone={() => {
            setImporting(null);
            setChecked(new Set());
          }}
        />
      )}
    </div>
  );
}

interface Row {
  secret: string;
  name: string;
  version: string;
}

function ImportDialog({
  target,
  secrets,
  versions,
  onLoadVersions,
  onClose,
  onDone,
}: {
  target: GcpTarget;
  secrets: string[];
  versions: Record<string, GcpSecretVersion[] | string>;
  onLoadVersions(secret: string): void;
  onClose(): void;
  onDone(): void;
}) {
  const project = useWorkspace((s) => s.project);
  const activeEnv = useWorkspace((s) => s.activeEnv());
  const go = useNav((s) => s.go);
  const envs = project?.environments ?? [];
  const [envId, setEnvId] = useState(activeEnv?.id ?? envs[0]?.id ?? '');
  const [mode, setMode] = useState<SecretImportMode>('keychain');
  const [rows, setRows] = useState<Row[]>(() => secrets.map((s) => ({ secret: s, name: envSecretName(s), version: 'latest' })));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const env = envs.find((e) => e.id === envId);

  useEffect(() => {
    if (secrets.length <= 20) secrets.forEach(onLoadVersions);
  }, [secrets]); // eslint-disable-line react-hooks/exhaustive-deps

  const run = async () => {
    if (!project || !envId) return;
    setBusy(true);
    setError(null);
    try {
      const r = await infraApi.gcp.secretsImport(
        project.root,
        envId,
        target,
        rows.map((x) => ({ secret: x.secret, name: x.name.trim(), version: x.version })),
        mode,
      );
      useVarsVersion.getState().bump();
      await useWorkspace.getState().refresh();
      if (r.imported.length) toast.success(`Imported ${r.imported.length} secret${r.imported.length === 1 ? '' : 's'} into ${r.environment}`, r.imported.join(', '));
      for (const [name, why] of r.skipped) toast.error(`Skipped ${name}`, why);
      for (const w of r.warnings) toast.info('Note', w);
      if (r.skipped.length === 0) onDone();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const names = rows.map((r) => r.name.trim());
  const invalid = names.some((n) => !/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(n)) || new Set(names).size !== names.length;

  return (
    <Modal
      open
      onClose={onClose}
      size="lg"
      title="Import secrets into an environment"
      footer={
        <>
          <span className="subtle xs grow">{mode === 'keychain' ? 'Values go to your OS keychain only — never into project files.' : 'Only references are written; each teammate resolves them with their own gcloud login.'}</span>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={() => void run()} disabled={busy || !env || invalid || rows.length === 0}>
            {busy ? <Spinner /> : mode === 'keychain' ? <KeyRound /> : <Link2 />} Import {rows.length}
          </button>
        </>
      }
    >
      <div className="col">
        {!project ? (
          <Callout kind="warning">
            Open a project first.{' '}
            <button className="btn btn-sm" onClick={() => go('projects')}>
              Projects
            </button>
          </Callout>
        ) : envs.length === 0 ? (
          <Callout kind="warning">This project has no environments yet — create one under Environments.</Callout>
        ) : (
          <>
            <div className="grid-2">
              <Field label="Environment">
                <select className="select" value={envId} onChange={(e) => setEnvId(e.target.value)}>
                  {envs.map((e) => (
                    <option key={e.id} value={e.id}>
                      {e.file.name}
                      {e.file.protected ? ' (protected)' : ''}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="Store as">
                <Segmented<SecretImportMode>
                  value={mode}
                  onChange={setMode}
                  options={[
                    { value: 'keychain', label: 'Values in the keychain', title: 'Read each value once and store it in the OS keychain for this environment' },
                    { value: 'reference', label: 'gcp-sm:// references', title: 'Add secretSources entries; values are fetched with gcloud when used' },
                  ]}
                />
              </Field>
            </div>
            <table className="table">
              <thead>
                <tr>
                  <th>Secret Manager</th>
                  <th>Version</th>
                  <th>Environment secret</th>
                  {mode === 'reference' && <th>Reference</th>}
                </tr>
              </thead>
              <tbody>
                {rows.map((r, i) => {
                  const vs = versions[r.secret];
                  return (
                    <tr key={r.secret}>
                      <td className="mono small">{r.secret}</td>
                      <td>
                        <select className="select sm" value={r.version} onChange={(e) => setRows((rs) => rs.map((x, j) => (j === i ? { ...x, version: e.target.value } : x)))}>
                          <option value="latest">latest</option>
                          {Array.isArray(vs) &&
                            vs
                              .filter((v) => v.state === 'ENABLED')
                              .map((v) => (
                                <option key={v.name} value={v.name}>
                                  {v.name}
                                </option>
                              ))}
                        </select>
                      </td>
                      <td>
                        <input className="input sm mono" value={r.name} onChange={(e) => setRows((rs) => rs.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)))} />
                      </td>
                      {mode === 'reference' && <td className="mono xs subtle break">{gcpReference(target.project, r.secret, r.version)}</td>}
                    </tr>
                  );
                })}
              </tbody>
            </table>
            {invalid && <Callout kind="warning">Names must be unique and start with a letter or _ (letters, digits, _ - . only).</Callout>}
            {env && (
              <span className="xs subtle">
                The names are added to <span className="mono">environments/{env.id}.env.yaml</span> → <span className="mono">secrets:</span>
                {mode === 'reference' ? ' and secretSources' : ''}. Use them as <span className="mono">{'{{secret.NAME}}'}</span>.
              </span>
            )}
            {error && <Callout kind="danger">{error}</Callout>}
          </>
        )}
      </div>
    </Modal>
  );
}
