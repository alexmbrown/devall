import { useEffect, useMemo, useRef, useState } from 'react';
import { openapiApi } from '@/lib/api/openapi';
import clsx from 'clsx';
import { ArrowLeft, FileUp, Folder, FolderOpen, KeyRound, Upload } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, Field, MethodBadge, Spinner, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import type { TreeNode } from '@/lib/types';
import { importApi, type ApplyReport, type DetectedSecret, type ImportSource, type PlanSummary, type SummaryNode } from '@/lib/api/import';
import { useWorkspace } from '@/store/workspace';
import './import.css';

export interface ImportDialogProps {
  open: boolean;
  onClose(): void;
  /** Folder (relative to `http/`) preselected as the destination; '' = project root. */
  targetFolder?: string;
}

const MAX_DROP_BYTES = 64 * 1024 * 1024;
const GENERIC_NAMES = ['HTTP requests', 'Hurl requests', 'cURL import', 'Bruno request', 'Postman environment'];
const FILE_EXTENSIONS = ['json', 'yaml', 'yml', 'har', 'http', 'rest', 'hurl', 'bru', 'txt', 'sh'];

type Step = 'source' | 'preview' | 'done';

/** Flat list of folders for the destination picker. */
function folderOptions(tree: TreeNode | null): { path: string; label: string }[] {
  const out = [{ path: '', label: 'Project root' }];
  const walk = (n: TreeNode, depth: number) => {
    if (n.kind !== 'folder') return;
    for (const c of n.children) {
      if (c.kind === 'folder') {
        out.push({ path: c.path, label: `${'  '.repeat(depth)}${c.name}` });
        walk(c, depth + 1);
      }
    }
  };
  if (tree) walk(tree, 1);
  return out;
}

function secretKey(s: DetectedSecret) {
  return `${s.environment}|${s.name}`;
}

/**
 * Import from Postman, Insomnia, Bruno, OpenCollection, OpenAPI/Swagger, HAR, .http, Hurl or cURL:
 * pick (or drop, or paste) a source → preview with warnings → import → optionally store the
 * detected credentials in the OS keychain.
 */
export function ImportDialog({ open, onClose, targetFolder = '' }: ImportDialogProps) {
  const project = useWorkspace((s) => s.project);
  const tree = useWorkspace((s) => s.tree);
  const [step, setStep] = useState<Step>('source');
  const [source, setSource] = useState<ImportSource | null>(null);
  const [sourceLabel, setSourceLabel] = useState('');
  const [fileStem, setFileStem] = useState<string | null>(null);
  const [pasted, setPasted] = useState('');
  const [detected, setDetected] = useState<string | null>(null);
  const [summary, setSummary] = useState<PlanSummary | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);

  const [target, setTarget] = useState(targetFolder);
  const [wrap, setWrap] = useState(true);
  const [folderName, setFolderName] = useState('');
  const [overwrite, setOverwrite] = useState(false);
  const [importEnvs, setImportEnvs] = useState(true);
  const [importVars, setImportVars] = useState(true);
  const [extractSecrets, setExtractSecrets] = useState(true);
  const [includeCookies, setIncludeCookies] = useState(false);
  const [includeAssets, setIncludeAssets] = useState(false);
  /** OpenAPI from a file: link the folder to the spec and re-sync it when the file changes. */
  const [keepInSync, setKeepInSync] = useState(true);

  const [report, setReport] = useState<ApplyReport | null>(null);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [typed, setTyped] = useState<Record<string, string>>({});
  const [storing, setStoring] = useState(false);
  const [stored, setStored] = useState(false);

  const folders = useMemo(() => folderOptions(tree), [tree]);

  useEffect(() => {
    if (!open) return;
    setStep('source');
    setSource(null);
    setSummary(null);
    setReport(null);
    setError(null);
    setPasted('');
    setDetected(null);
    setTarget(targetFolder);
    setStored(false);
  }, [open, targetFolder]);

  // Live format badge while pasting.
  const detectTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => {
    clearTimeout(detectTimer.current);
    if (!pasted.trim()) return setDetected(null);
    detectTimer.current = setTimeout(() => {
      importApi
        .detect(pasted, project?.root)
        .then((d) => setDetected(d.label))
        .catch(() => setDetected(null));
    }, 250);
    return () => clearTimeout(detectTimer.current);
  }, [pasted]);

  const preview = async (src: ImportSource, label: string, stem: string | null) => {
    setBusy(true);
    setError(null);
    try {
      let s: PlanSummary;
      try {
        s = await importApi.preview(src);
      } catch (e) {
        // Not a built-in format: a plugin importer may know it.
        const d = await importApi.detect(src.path ?? src.text ?? '', project?.root).catch(() => null);
        if (!d?.plugin) throw e;
        src = { ...src, plugin: d.plugin.id, root: project?.root };
        s = await importApi.preview(src);
      }
      setSource(src);
      setSourceLabel(label);
      setFileStem(stem);
      setSummary(s);
      setFolderName(stem && GENERIC_NAMES.includes(s.name) ? stem : s.name);
      setImportEnvs(true);
      setImportVars(true);
      setStep('preview');
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const withOptions = (src: ImportSource): ImportSource => ({ ...src, options: { includeCookies, includeAssets } });

  const chooseFile = async () => {
    const picked = await openDialog({
      multiple: false,
      directory: false,
      filters: [
        { name: 'API collections and requests', extensions: FILE_EXTENSIONS },
        { name: 'All files', extensions: ['*'] },
      ],
    });
    if (typeof picked === 'string') preview(withOptions({ path: picked }), picked, null);
  };

  const chooseFolder = async () => {
    const picked = await openDialog({ directory: true, multiple: false, title: 'Choose a Bruno or OpenCollection folder' });
    if (typeof picked === 'string') preview(withOptions({ path: picked }), picked, null);
  };

  const onDrop = async (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);
    const file = e.dataTransfer.files[0];
    if (!file) return;
    if (file.size > MAX_DROP_BYTES) return setError(`${file.name} is too large to import (${Math.round(file.size / 1024 / 1024)} MB).`);
    try {
      const text = await file.text();
      preview(withOptions({ text }), file.name, file.name.replace(/\.[^.]+$/, '').replace(/\.(postman_collection|postman_environment|insomnia)$/, ''));
    } catch (err) {
      setError(`Could not read ${file.name}: ${errorMessage(err)}`);
    }
  };

  // HAR options change what gets imported: re-run the preview.
  useEffect(() => {
    if (step !== 'preview' || !source || summary?.format !== 'har') return;
    const src = withOptions(source);
    importApi
      .preview(src)
      .then((s) => {
        setSource(src);
        setSummary(s);
      })
      .catch((e) => setError(errorMessage(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [includeCookies, includeAssets]);

  const doImport = async () => {
    if (!project || !source) return;
    setBusy(true);
    setError(null);
    try {
      const r = await importApi.apply(project.root, source, target, {
        wrapInFolder: wrap,
        folderName: wrap ? folderName.trim() || null : null,
        overwrite,
        importEnvironments: importEnvs,
        importVariables: importVars,
        extractSecrets,
      });
      setReport(r);
      const isSpec = summary?.format === 'openApi' || summary?.format === 'swagger';
      if (isSpec && keepInSync && source.path && r.folder) {
        await openapiApi
          .link(project.root, r.folder, source.path, true)
          .then(() => toast.info('Linked to the spec', `http/${r.folder} re-syncs when ${source.path} changes`))
          .catch((e) => toast.error('Could not link the spec', errorMessage(e)));
      }
      setChosen(new Set(r.secrets.filter((s) => s.value).map(secretKey)));
      setTyped({});
      setStep('done');
      await useWorkspace.getState().refresh();
      if (r.secrets.length === 0) toast.success(`Imported ${r.requestsCreated + r.requestsUpdated} requests`);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const storeSecrets = async () => {
    if (!project || !report) return;
    setStoring(true);
    let ok = 0;
    try {
      for (const s of report.secrets) {
        const key = secretKey(s);
        if (!chosen.has(key)) continue;
        const value = s.value || typed[key] || '';
        if (!value) continue;
        await api.setSecret(project.root, s.environment, s.name, value);
        ok++;
      }
      setStored(true);
      toast.success(`Stored ${ok} secret${ok === 1 ? '' : 's'} in the keychain`);
    } catch (e) {
      toast.error('Could not store secrets', errorMessage(e));
    } finally {
      setStoring(false);
    }
  };

  const envName = (id: string) => (id === '*' ? 'All environments' : (project?.environments.find((e) => e.id === id)?.file.name ?? id));

  const close = () => {
    if (!busy && !storing) onClose();
  };

  const footer =
    step === 'source' ? (
      <>
        <span className="grow" />
        <button className="btn" onClick={close}>
          Cancel
        </button>
        <button className="btn btn-primary" disabled={!pasted.trim() || busy} onClick={() => preview(withOptions({ text: pasted }), 'Pasted text', null)}>
          {busy ? <Spinner /> : null} Preview
        </button>
      </>
    ) : step === 'preview' ? (
      <>
        <button className="btn" onClick={() => setStep('source')} disabled={busy}>
          <ArrowLeft /> Back
        </button>
        <span className="grow" />
        <button className="btn" onClick={close}>
          Cancel
        </button>
        <button className="btn btn-primary" onClick={doImport} disabled={busy || !summary || (!summary.requestCount && !summary.environments.length && !summary.projectVariables.length)}>
          {busy ? <Spinner /> : <Upload />} Import
        </button>
      </>
    ) : (
      <>
        {report?.firstRequest && (
          <button
            className="btn"
            onClick={() => {
              useWorkspace.getState().openRequest(report.firstRequest!);
              onClose();
            }}
          >
            Open first request
          </button>
        )}
        <span className="grow" />
        {report && report.secrets.length > 0 && !stored && (
          <button className="btn" onClick={onClose} disabled={storing}>
            Skip
          </button>
        )}
        {report && report.secrets.length > 0 && !stored ? (
          <button className="btn btn-primary" onClick={storeSecrets} disabled={storing || chosen.size === 0}>
            {storing ? <Spinner /> : <KeyRound />} Store {chosen.size} in keychain
          </button>
        ) : (
          <button className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        )}
      </>
    );

  return (
    <Modal open={open} onClose={close} title="Import" size="lg" footer={footer}>
      {!project ? (
        <Callout kind="warning">Open a project first.</Callout>
      ) : step === 'source' ? (
        <div className="imp-col">
          <div
            className={clsx('drop-zone imp-drop', dragOver && 'over')}
            onDragOver={(e) => {
              e.preventDefault();
              setDragOver(true);
            }}
            onDragLeave={() => setDragOver(false)}
            onDrop={onDrop}
            onClick={chooseFile}
            role="button"
            tabIndex={0}
            onKeyDown={(e) => (e.key === 'Enter' || e.key === ' ') && chooseFile()}
          >
            <FileUp />
            <div className="strong">Drop a file here, or click to choose one</div>
            <div className="small muted">Exports are read locally — nothing is uploaded.</div>
          </div>
          <div className="imp-row">
            <button className="btn btn-sm" onClick={chooseFile} disabled={busy}>
              <FileUp /> Choose file…
            </button>
            <button className="btn btn-sm" onClick={chooseFolder} disabled={busy} title="Bruno (.bru) or OpenCollection (YAML) collection folder">
              <FolderOpen /> Choose folder…
            </button>
            {busy && <Spinner />}
          </div>
          <Field label={<span className="imp-row">Or paste {detected && <span className="badge accent">{detected}</span>}</span>} hint="cURL commands, a Postman/Insomnia/OpenAPI document, a .http or Hurl file…">
            <textarea className="textarea imp-paste" value={pasted} onChange={(e) => setPasted(e.target.value)} placeholder="curl https://api.example.com/users -H 'Accept: application/json'" spellCheck={false} />
          </Field>
          {error && <Callout kind="danger">{error}</Callout>}
          <div className="imp-formats">
            Supported: Postman collections (v2.0, v2.1) and environments · Insomnia (v4 JSON, v5 YAML) · Bruno folders and .bru files · OpenCollection YAML ·
            OpenAPI 3.0 / 3.1 and Swagger 2.0 (JSON or YAML) · HAR · .http / .rest (JetBrains, VS Code REST Client) · Hurl · cURL
          </div>
        </div>
      ) : step === 'preview' && summary ? (
        <PreviewStep
          summary={summary}
          sourceLabel={sourceLabel}
          fileStem={fileStem}
          error={error}
          folders={folders}
          target={target}
          setTarget={setTarget}
          wrap={wrap}
          setWrap={setWrap}
          folderName={folderName}
          setFolderName={setFolderName}
          overwrite={overwrite}
          setOverwrite={setOverwrite}
          importEnvs={importEnvs}
          setImportEnvs={setImportEnvs}
          importVars={importVars}
          setImportVars={setImportVars}
          extractSecrets={extractSecrets}
          setExtractSecrets={setExtractSecrets}
          includeCookies={includeCookies}
          setIncludeCookies={setIncludeCookies}
          includeAssets={includeAssets}
          setIncludeAssets={setIncludeAssets}
          canLink={!!source?.path}
          keepInSync={keepInSync}
          setKeepInSync={setKeepInSync}
        />
      ) : report ? (
        <div className="imp-col">
          <Callout kind="success">
            Imported {report.requestsCreated} request{report.requestsCreated === 1 ? '' : 's'}
            {report.requestsUpdated > 0 && <>, updated {report.requestsUpdated}</>}
            {report.foldersCreated > 0 && <> in {report.foldersCreated} new folder{report.foldersCreated === 1 ? '' : 's'}</>}
            {report.folder && (
              <>
                {' '}
                under <code className="code-inline">http/{report.folder}</code>
              </>
            )}
            {report.environmentsCreated.length + report.environmentsUpdated.length > 0 && (
              <>
                {' '}
                · {report.environmentsCreated.length} environment{report.environmentsCreated.length === 1 ? '' : 's'} created
                {report.environmentsUpdated.length > 0 && <>, {report.environmentsUpdated.length} updated</>}
              </>
            )}
            {report.variablesAdded.length > 0 && <> · {report.variablesAdded.length} project variables</>}.
          </Callout>
          {report.secrets.length > 0 && (
            <div className="imp-col" style={{ gap: 'var(--space-3)' }}>
              <div className="imp-section-title">Secrets</div>
              <div className="small muted">
                These values looked like credentials, so the imported files reference them as <code className="code-inline">{'{{secret.NAME}}'}</code> instead. Store them in
                the OS keychain now — they were not written to any project file.
              </div>
              <table className="table imp-secrets">
                <thead>
                  <tr>
                    <th style={{ width: 28 }} />
                    <th>Name</th>
                    <th>Environment</th>
                    <th>Found in</th>
                    <th>Value</th>
                  </tr>
                </thead>
                <tbody>
                  {report.secrets.map((s) => {
                    const key = secretKey(s);
                    const on = chosen.has(key);
                    return (
                      <tr key={key}>
                        <td>
                          <Checkbox
                            checked={on}
                            onChange={(v) =>
                              setChosen((cur) => {
                                const next = new Set(cur);
                                if (v) next.add(key);
                                else next.delete(key);
                                return next;
                              })
                            }
                          />
                        </td>
                        <td className="mono small">{s.name}</td>
                        <td className="small">{envName(s.environment)}</td>
                        <td className="small muted truncate" title={s.source}>
                          {s.source}
                        </td>
                        <td>
                          {s.value ? (
                            <span className="value">••••••</span>
                          ) : (
                            <input
                              className="input sm"
                              type="password"
                              placeholder="not in the export — type it"
                              value={typed[key] ?? ''}
                              onChange={(e) => {
                                const v = e.target.value;
                                setTyped((t) => ({ ...t, [key]: v }));
                                if (v) setChosen((cur) => new Set(cur).add(key));
                              }}
                              disabled={stored}
                            />
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
              {stored && <Callout kind="success">Stored. Manage them later in Environments.</Callout>}
            </div>
          )}
          {report.warnings.length > 0 && (
            <Callout kind="warning">
              <ul className="imp-list">
                {report.warnings.map((w) => (
                  <li key={w}>{w}</li>
                ))}
              </ul>
            </Callout>
          )}
        </div>
      ) : null}
    </Modal>
  );
}

interface PreviewProps {
  summary: PlanSummary;
  sourceLabel: string;
  fileStem: string | null;
  error: string | null;
  folders: { path: string; label: string }[];
  target: string;
  setTarget(v: string): void;
  wrap: boolean;
  setWrap(v: boolean): void;
  folderName: string;
  setFolderName(v: string): void;
  overwrite: boolean;
  setOverwrite(v: boolean): void;
  importEnvs: boolean;
  setImportEnvs(v: boolean): void;
  importVars: boolean;
  setImportVars(v: boolean): void;
  extractSecrets: boolean;
  setExtractSecrets(v: boolean): void;
  includeCookies: boolean;
  setIncludeCookies(v: boolean): void;
  includeAssets: boolean;
  setIncludeAssets(v: boolean): void;
  /** The source is a file (an OpenAPI file can be linked). */
  canLink: boolean;
  keepInSync: boolean;
  setKeepInSync(v: boolean): void;
}

function PreviewStep(p: PreviewProps) {
  const s = p.summary;
  return (
    <div className="imp-col">
      <div className="imp-row">
        <span className="badge accent">{s.formatLabel}</span>
        <span className="strong truncate" title={p.sourceLabel}>
          {s.name}
        </span>
        <span className="small muted">
          {s.requestCount} request{s.requestCount === 1 ? '' : 's'} · {s.folderCount} folder{s.folderCount === 1 ? '' : 's'} · {s.environments.length} environment
          {s.environments.length === 1 ? '' : 's'}
        </span>
      </div>
      {p.error && <Callout kind="danger">{p.error}</Callout>}
      <div className="imp-split">
        <div className="imp-col" style={{ gap: 'var(--space-4)' }}>
          {s.tree.length > 0 ? (
            <div className="imp-tree">
              {s.tree.map((n, i) => (
                <PreviewNode key={i} node={n} depth={0} />
              ))}
            </div>
          ) : (
            <div className="small muted">No requests in this file.</div>
          )}
          {s.environments.length > 0 && (
            <div className="imp-col" style={{ gap: 'var(--space-2)' }}>
              <div className="imp-section-title">Environments</div>
              {s.environments.map((e) => (
                <div className="imp-env" key={e.name}>
                  <span>{e.name}</span>
                  <span className="count">
                    {e.variables.length} variable{e.variables.length === 1 ? '' : 's'}
                    {e.secrets.length > 0 && <> · {e.secrets.length} secret{e.secrets.length === 1 ? '' : 's'}</>}
                  </span>
                </div>
              ))}
            </div>
          )}
          {s.secrets.length > 0 && p.extractSecrets && (
            <Callout kind="info">
              {s.secrets.length} value{s.secrets.length === 1 ? '' : 's'} look like credentials ({[...new Set(s.secrets.map((x) => x.name))].slice(0, 6).join(', ')}
              {s.secrets.length > 6 ? ', …' : ''}). They become <code className="code-inline">{'{{secret.NAME}}'}</code> references and you can store the values in the
              keychain after importing — they never land in project files.
            </Callout>
          )}
          {s.warnings.length > 0 && (
            <Callout kind="warning">
              <ul className="imp-list">
                {s.warnings.map((w) => (
                  <li key={w}>{w}</li>
                ))}
              </ul>
            </Callout>
          )}
        </div>
        <div className="imp-options">
          <Field label="Import into">
            <select className="select" value={p.target} onChange={(e) => p.setTarget(e.target.value)}>
              {p.folders.map((f) => (
                <option key={f.path} value={f.path}>
                  {f.label}
                </option>
              ))}
            </select>
          </Field>
          <Switch checked={p.wrap} onChange={p.setWrap} label="Put it in a new folder" />
          {p.wrap && (
            <Field label="Folder name">
              <input className="input" value={p.folderName} onChange={(e) => p.setFolderName(e.target.value)} placeholder={p.fileStem ?? s.name} />
            </Field>
          )}
          {s.environments.length > 0 && <Checkbox checked={p.importEnvs} onChange={p.setImportEnvs} label={`Import ${s.environments.length} environment${s.environments.length === 1 ? '' : 's'}`} />}
          {s.projectVariables.length > 0 && (
            <Checkbox checked={p.importVars} onChange={p.setImportVars} label={`Add ${s.projectVariables.length} project variable${s.projectVariables.length === 1 ? '' : 's'}`} />
          )}
          <Checkbox checked={p.extractSecrets} onChange={p.setExtractSecrets} label="Move credentials to the keychain" title="Turn tokens, passwords and API keys into {{secret.NAME}} references" />
          <Checkbox
            checked={p.overwrite}
            onChange={p.setOverwrite}
            label="Update existing items"
            title="Same-named folders, requests and environments are updated in place (keeps their ids) instead of imported as copies — handy to re-sync an OpenAPI spec"
          />
          {(s.format === 'openApi' || s.format === 'swagger') && p.canLink && p.wrap && (
            <Checkbox
              checked={p.keepInSync}
              onChange={p.setKeepInSync}
              label="Keep the folder in sync with the spec file"
              title="Stores the spec path in the folder's _folder.yaml; the folder is re-imported in place when the file changes (Linked spec in the folder menu)"
            />
          )}
          {s.format === 'har' && (
            <>
              <div className="imp-section-title">HAR</div>
              <Checkbox checked={p.includeCookies} onChange={p.setIncludeCookies} label="Keep cookies (stored as secrets)" />
              <Checkbox checked={p.includeAssets} onChange={p.setIncludeAssets} label="Keep scripts, styles and images" />
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function PreviewNode({ node, depth }: { node: SummaryNode; depth: number }) {
  const pad = { paddingLeft: 8 + depth * 14 };
  if (node.kind === 'folder') {
    return (
      <>
        <div className="imp-tree-row folder" style={pad} title={node.hasSettings ? 'Has folder headers, auth or variables' : undefined}>
          <Folder />
          <span className="truncate">{node.name}</span>
          {node.hasSettings && <span className="badge outline">settings</span>}
        </div>
        {node.children.map((c, i) => (
          <PreviewNode key={i} node={c} depth={depth + 1} />
        ))}
      </>
    );
  }
  return (
    <div className="imp-tree-row" style={pad}>
      <MethodBadge method={node.method} short />
      <span className="truncate">{node.name}</span>
      <span className="url truncate" title={node.url}>
        {node.url}
      </span>
    </div>
  );
}
