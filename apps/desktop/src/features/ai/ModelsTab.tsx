import { useCallback, useEffect, useState } from 'react';
import clsx from 'clsx';
import { Cpu, Download, HardDrive, Pencil, Play, Plus, Radar, RefreshCw, Server, Square, Star, Trash2, Wifi } from 'lucide-react';
import { Callout, Field, Spinner, Switch } from '@/components/ui/misc';
import { Modal } from '@/components/ui/Modal';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import {
  aiApi,
  formatGiB,
  isLoopback,
  MANAGED_PROVIDER_ID,
  PROVIDER_PRESETS,
  type CatalogEntry,
  type CatalogView,
  type DiscoveredServer,
  type HardwareView,
  type ProviderConfig,
  type ProviderKind,
  type SidecarStatus,
} from '@/lib/api/ai';
import { errorMessage } from '@/lib/ipc';
import { isLocalProvider, useAi } from './store';
import { downloadView, useAiDownloads, type DownloadView } from './downloads';

const ROLE_LABEL: Record<string, string> = { general: 'General', code: 'Code + FIM', fim: 'Autocomplete', embedding: 'Embeddings' };
const KIND_LABEL: Record<ProviderKind, string> = { openAiCompatible: 'OpenAI-compatible', ollama: 'Ollama', anthropic: 'Anthropic', gemini: 'Gemini' };

export function ModelsTab() {
  const { view, load } = useAi();
  const [hw, setHw] = useState<HardwareView | null>(null);
  const [hwBusy, setHwBusy] = useState(false);
  const [catalog, setCatalog] = useState<CatalogView | null>(null);
  const [sidecar, setSidecar] = useState<SidecarStatus | null>(null);
  const downloads = useAiDownloads((s) => s.byKey);
  const finished = useAiDownloads((s) => s.finished);
  const [found, setFound] = useState<DiscoveredServer[] | null>(null);
  const [scanning, setScanning] = useState(false);
  const [editing, setEditing] = useState<ProviderConfig | 'new' | null>(null);
  const [starting, setStarting] = useState<string | null>(null);

  const refreshCatalog = useCallback(() => {
    aiApi.catalog().then(setCatalog).catch((e) => toast.error('Could not read the model catalog', errorMessage(e)));
    aiApi.sidecarStatus().then(setSidecar).catch(() => {});
  }, []);

  const detect = useCallback(async (refresh: boolean) => {
    setHwBusy(true);
    try {
      setHw(await aiApi.hardware(refresh));
    } catch (e) {
      toast.error('Hardware detection failed', errorMessage(e));
    } finally {
      setHwBusy(false);
    }
  }, []);

  useEffect(() => {
    load();
    detect(false);
    void useAiDownloads.getState().hydrate();
  }, [load, detect]);
  // Re-read file status on mount and whenever a download ends (wherever it was started from).
  useEffect(() => refreshCatalog(), [refreshCatalog, finished]);

  const download = async (m: CatalogEntry, resume: boolean) => {
    if (resume) return useAiDownloads.getState().start(m.id, m.name, () => aiApi.downloadModel(m.id));
    const ok = await confirmDialog({
      title: `Download ${m.name}?`,
      message: (
        <>
          {formatGiB(m.sizeBytes)} from <span className="mono xs">huggingface.co/{m.hfRepo}</span> ({m.license}). The file is verified with SHA-256 and stored in
          this app's data folder. You can pause and resume.
        </>
      ),
      confirmText: 'Download',
    });
    if (!ok) return;
    await useAiDownloads.getState().start(m.id, m.name, () => aiApi.downloadModel(m.id));
  };

  const installRuntime = async () => {
    if (!hw) return;
    const ok = await confirmDialog({
      title: `Install llama.cpp ${catalog?.llamaBuild ?? ''}?`,
      message: (
        <>
          {hw.llama.reason}. Downloads {formatGiB(hw.llamaDownloadBytes)} from the official ggml-org/llama.cpp GitHub release, verified with a pinned SHA-256. Some antivirus
          products flag llama.cpp binaries by mistake; if yours does, allow the folder or use Ollama / LM Studio instead.
        </>
      ),
      confirmText: 'Install',
    });
    if (!ok) return;
    await useAiDownloads.getState().start(hw.llamaKey, 'llama.cpp', () => aiApi.installRuntime(hw.llama.variant));
  };

  const start = async (m: CatalogEntry) => {
    setStarting(m.id);
    try {
      setSidecar(await aiApi.sidecarStart(m.id));
      await load();
      toast.success(`${m.name} is running`, 'Use it as the “Managed” provider.');
    } catch (e) {
      toast.error(`Could not start ${m.name}`, errorMessage(e));
    } finally {
      setStarting(null);
    }
  };

  const stop = async () => {
    await aiApi.sidecarStop().catch(() => {});
    await load();
    refreshCatalog();
  };

  const scan = async () => {
    setScanning(true);
    try {
      setFound(await aiApi.discover());
    } finally {
      setScanning(false);
    }
  };

  const config = view?.config;
  const recommended = new Set(hw ? [hw.recommendation.general, hw.recommendation.fim, hw.recommendation.embedding, ...(hw.recommendation.code ? [hw.recommendation.code] : [])] : []);
  const runtimeKey = hw?.llamaKey ?? '';
  const runtime = downloadView(downloads[runtimeKey], { downloaded: !!catalog?.server, partialBytes: 0, sizeBytes: hw?.llamaDownloadBytes ?? null });

  return (
    <div className="col-lg" style={{ maxWidth: 1000, margin: '0 auto', width: '100%' }}>
      <Callout kind="success">
        Everything here runs on your machine — no account, no telemetry. Downloads only start when you click, and cloud providers are optional.
      </Callout>

      {/* ── This machine ── */}
      <section className="ai-section">
        <h2>
          <Cpu /> This machine
          <span className="spacer" />
          <button className="btn btn-sm btn-ghost" onClick={() => detect(true)} disabled={hwBusy}>
            {hwBusy ? <Spinner /> : <RefreshCw />} Re-detect
          </button>
        </h2>
        {hw ? (
          <>
            <div className="ai-hw">
              <div className="card">
                <div className="ai-hw-label">Memory</div>
                <div className="ai-hw-value">{formatGiB(hw.hardware.totalRamBytes)}</div>
                <div className="xs subtle">{hw.hardware.appleSilicon ? 'unified (Apple Silicon)' : hw.hardware.arch}</div>
              </div>
              <div className="card">
                <div className="ai-hw-label">GPU</div>
                <div className="ai-hw-value">{hw.hardware.gpus.length ? hw.hardware.gpus.map((g) => g.name).join(', ') : 'none detected'}</div>
                <div className="xs subtle">{hw.hardware.gpus.map((g) => (g.vramBytes ? `${formatGiB(g.vramBytes)} VRAM` : '')).filter(Boolean).join(', ')}</div>
              </div>
              <div className="card">
                <div className="ai-hw-label">Tier</div>
                <div className="ai-hw-value">{hw.tierLabel}</div>
                <div className="xs subtle">{hw.hardware.cpu ?? hw.hardware.os}</div>
              </div>
              <div className="card">
                <div className="ai-hw-label">Recommended</div>
                <div className="ai-hw-value">{catalog?.models.find((m) => m.id === hw.recommendation.general)?.name ?? hw.recommendation.general}</div>
                <div className="xs subtle">llama.cpp: {hw.llama.variant}</div>
              </div>
            </div>
            {[...hw.hardware.notes, ...hw.recommendation.notes].map((n) => (
              <div key={n} className="xs subtle">
                {n}
              </div>
            ))}
          </>
        ) : (
          <div className="row subtle small">{hwBusy ? <Spinner /> : null} Detecting memory and GPUs…</div>
        )}
      </section>

      {/* ── Providers ── */}
      <section className="ai-section">
        <h2>
          <Server /> Model providers
          <span className="spacer" />
          <button className="btn btn-sm" onClick={() => setEditing('new')}>
            <Plus /> Add provider
          </button>
        </h2>
        <div className="panel">
          {view?.managed && (
            <ProviderRow
              p={view.managed}
              isDefault={config?.defaultProvider === MANAGED_PROVIDER_ID}
              onDefault={async () => {
                await aiApi.settings({ defaultProvider: MANAGED_PROVIDER_ID });
                load();
              }}
            />
          )}
          {config?.providers.map((p) => (
            <ProviderRow
              key={p.id}
              p={p}
              isDefault={config.defaultProvider === p.id}
              onDefault={async () => {
                await aiApi.settings({ defaultProvider: p.id });
                load();
              }}
              onEdit={() => setEditing(p)}
              onRemove={async () => {
                if (await confirmDialog({ title: `Remove ${p.name}?`, message: 'Its API key is removed from the OS keychain too.', confirmText: 'Remove', danger: true })) {
                  await aiApi.removeProvider(p.id);
                  load();
                }
              }}
            />
          ))}
          {!view?.managed && !config?.providers.length && <div className="p-4 subtle small">No providers yet — scan for Ollama / LM Studio below, add one, or start a managed model.</div>}
        </div>
        <Switch
          checked={!!config?.allowRemoteContext}
          onChange={async (v) => {
            await aiApi.settings({ allowRemoteContext: v });
            load();
          }}
          label={
            <span className="small">
              Allow project notes and to-dos to be sent to <strong>cloud</strong> providers{' '}
              <span className="subtle">(off: Ask and Summarize only use local models; secrets are always masked)</span>
            </span>
          }
        />
        {view && !view.keychainAvailable && <Callout kind="warning">The OS keychain is unavailable: API keys are kept in memory for this session only.</Callout>}
      </section>

      {/* ── Discovery ── */}
      <section className="ai-section">
        <h2>
          <Radar /> Local servers
          <span className="spacer" />
          <button className="btn btn-sm" onClick={scan} disabled={scanning}>
            {scanning ? <Spinner /> : <Wifi />} Scan this machine
          </button>
        </h2>
        <div className="small subtle">Looks for Ollama (:11434), LM Studio (:1234), a llama.cpp server (:8080) and Jan (:1337) on 127.0.0.1 only.</div>
        {found && found.length === 0 && <div className="small subtle">Nothing found. Start Ollama or LM Studio and scan again.</div>}
        {found?.map((s) => (
          <DiscoveredRow key={s.baseUrl} s={s} onAdded={load} />
        ))}
      </section>

      {/* ── Managed llama.cpp ── */}
      <section className="ai-section">
        <h2>
          <HardDrive /> Built-in runtime (llama.cpp {catalog?.llamaBuild})
        </h2>
        <div className="panel">
          <div className="ai-row">
            <div className="grow">
              <div className="ai-row-title">
                llama-server
                {catalog?.server ? <span className="badge success">installed</span> : <span className="badge outline">not installed</span>}
              </div>
              <div className="ai-row-sub" title={catalog?.server ?? ''}>
                {catalog?.server ?? (hw ? `${hw.llama.reason} · ${formatGiB(hw.llamaDownloadBytes)}` : 'Optional: run models without installing anything else')}
              </div>
            </div>
            {runtime.kind === 'active' ? (
              <DlProgress d={runtime} onCancel={() => useAiDownloads.getState().pause(runtimeKey)} />
            ) : (
              !catalog?.server &&
              hw && (
                <div className="ai-row-actions">
                  {runtime.kind !== 'ready' && runtime.error && (
                    <span className="xs text-danger truncate" title={runtime.error}>
                      {runtime.error}
                    </span>
                  )}
                  <button className="btn btn-sm" onClick={installRuntime}>
                    <Download /> {runtime.kind !== 'ready' && runtime.error ? 'Retry' : 'Install'}
                  </button>
                </div>
              )
            )}
          </div>
          {sidecar?.running && sidecar.info && (
            <div className="ai-row">
              <span className="dot ok pulse" />
              <div className="grow">
                <div className="ai-row-title">Running {sidecar.info.model}</div>
                <div className="ai-row-sub">
                  {sidecar.info.baseUrl} · pid {sidecar.info.pid ?? '?'} · API key required · logs: {sidecar.info.logFile}
                </div>
              </div>
              <button className="btn btn-sm" onClick={stop}>
                <Square /> Stop
              </button>
            </div>
          )}
        </div>
        <div className="panel">
          {catalog?.models.map((m) => {
            const d = downloadView(downloads[m.id], { downloaded: m.status.downloaded, partialBytes: m.status.partialBytes, sizeBytes: m.sizeBytes });
            const running = sidecar?.running && sidecar.info?.modelPath === m.status.path;
            return (
              <div key={m.id} className="ai-row">
                <div className="grow">
                  <div className="ai-row-title">
                    {recommended.has(m.id) && <Star size={13} className="ai-star" aria-label="Recommended for this machine" />}
                    {m.name}
                    <span className="badge">{ROLE_LABEL[m.role]}</span>
                    <span className="badge outline">{m.quant}</span>
                    {hw && m.tiers.includes(hw.tier) && <span className="badge accent">fits</span>}
                    {m.status.downloaded && <span className="badge success">downloaded</span>}
                    {running && <span className="badge success">running</span>}
                  </div>
                  <div className="ai-row-sub" title={m.notes}>
                    {formatGiB(m.sizeBytes)} · {m.license} · {m.hfRepo} · {m.notes}
                    {m.ollamaTag && ` · Ollama: ${m.ollamaTag}`}
                  </div>
                </div>
                {d.kind === 'active' ? (
                  <DlProgress d={d} onCancel={() => useAiDownloads.getState().pause(m.id)} />
                ) : d.kind === 'ready' ? (
                  <div className="ai-row-actions">
                    {m.role !== 'embedding' && !running && (
                      <button className="btn btn-sm btn-primary" onClick={() => start(m)} disabled={!!starting || !catalog.server} title={catalog.server ? 'Start llama-server with this model' : 'Install llama.cpp first'}>
                        {starting === m.id ? <Spinner /> : <Play />} Start
                      </button>
                    )}
                    <button
                      className="icon-btn"
                      title="Move to trash"
                      aria-label={`Delete ${m.name}`}
                      onClick={async () => {
                        if (await confirmDialog({ title: `Delete ${m.name}?`, message: `${formatGiB(m.sizeBytes)} goes to the trash.`, confirmText: 'Delete', danger: true })) {
                          await aiApi.deleteModel(m.id).catch((e) => toast.error('Delete failed', errorMessage(e)));
                          refreshCatalog();
                        }
                      }}
                    >
                      <Trash2 />
                    </button>
                  </div>
                ) : (
                  <div className="ai-row-actions">
                    {d.error && (
                      <span className="xs text-danger truncate" title={d.error}>
                        {d.error}
                      </span>
                    )}
                    <button className="btn btn-sm" onClick={() => download(m, d.kind === 'resume')} title={d.kind === 'resume' ? 'Continue where the download stopped' : undefined}>
                      <Download /> {d.kind === 'resume' ? `Resume (${d.pct}%)` : d.error ? 'Retry' : 'Download'}
                    </button>
                  </div>
                )}
              </div>
            );
          })}
        </div>
        <div className="xs subtle">
          Curated Apache-2.0 models for the Oct 2026 lineup. Files come from Hugging Face and are checked against pinned SHA-256 digests. Prefer Ollama on Macs for
          MLX speed.
        </div>
      </section>

      {editing && (
        <ProviderModal
          initial={editing === 'new' ? undefined : editing}
          existingIds={config?.providers.map((p) => p.id) ?? []}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            load();
          }}
        />
      )}
    </div>
  );
}

function DlProgress({ d, onCancel }: { d: Extract<DownloadView, { kind: 'active' }>; onCancel(): void }) {
  const busy = d.phase === 'verifying' || d.phase === 'installing';
  return (
    <div className="ai-row-actions">
      <div className="col-sm ai-dl">
        <div className="row-sm ai-dl-head">
          <span className="xs strong">{d.phase === 'verifying' ? 'Verifying' : d.phase === 'installing' ? 'Installing' : d.phase === 'starting' ? 'Starting' : 'Downloading'}</span>
          {!busy && <span className="xs mono">{Math.floor(d.pct)}%</span>}
        </div>
        <div className={clsx('ai-progress', busy && 'indeterminate')} role="progressbar" aria-valuenow={Math.floor(d.pct)} aria-valuemin={0} aria-valuemax={100}>
          <span style={{ width: `${busy ? 100 : d.pct}%` }} />
        </div>
        <div className="xs subtle">
          {d.phase === 'verifying'
            ? 'checking SHA-256…'
            : d.phase === 'installing'
              ? 'unpacking…'
              : `${formatGiB(d.downloaded)} / ${formatGiB(d.total)}${d.rate ? ` · ${formatGiB(d.rate)}/s` : ''}`}
        </div>
      </div>
      <button className="btn btn-sm btn-ghost" onClick={onCancel} disabled={busy} title={busy ? 'Almost done' : 'Pause — the partial download is kept'}>
        Pause
      </button>
    </div>
  );
}

function ProviderRow({ p, isDefault, onDefault, onEdit, onRemove }: { p: ProviderConfig; isDefault: boolean; onDefault(): void; onEdit?(): void; onRemove?(): void }) {
  const [testing, setTesting] = useState(false);
  const local = isLocalProvider(p);
  const test = async () => {
    setTesting(true);
    try {
      const models = await aiApi.testProvider({ id: p.id });
      toast.success(`${p.name} answered`, models.length ? `${models.length} model(s): ${models.slice(0, 6).join(', ')}${models.length > 6 ? '…' : ''}` : 'No models listed.');
    } catch (e) {
      toast.error(`${p.name} did not answer`, errorMessage(e));
    } finally {
      setTesting(false);
    }
  };
  return (
    <div className="ai-row">
      <button className={clsx('icon-btn', isDefault && 'on')} onClick={onDefault} title={isDefault ? 'Default provider' : 'Make default'} aria-label="Make default">
        <Star className={isDefault ? 'ai-star' : undefined} fill={isDefault ? 'currentColor' : 'none'} />
      </button>
      <div className="grow">
        <div className="ai-row-title">
          {p.name}
          <span className={clsx('badge', local ? 'success' : 'warning')}>{local ? 'local' : 'cloud'}</span>
          <span className="badge outline">{KIND_LABEL[p.kind]}</span>
          {p.hasApiKey && <span className="badge">key in keychain</span>}
        </div>
        <div className="ai-row-sub">
          {p.model} · {p.baseUrl}
        </div>
      </div>
      <button className="btn btn-sm btn-ghost" onClick={test} disabled={testing}>
        {testing ? <Spinner /> : <Wifi />} Test
      </button>
      {onEdit && (
        <button className="icon-btn" onClick={onEdit} title="Edit" aria-label={`Edit ${p.name}`}>
          <Pencil />
        </button>
      )}
      {onRemove && (
        <button className="icon-btn" onClick={onRemove} title="Remove" aria-label={`Remove ${p.name}`}>
          <Trash2 />
        </button>
      )}
    </div>
  );
}

function DiscoveredRow({ s, onAdded }: { s: DiscoveredServer; onAdded(): void }) {
  const [model, setModel] = useState(s.suggested.model);
  const add = async () => {
    try {
      await aiApi.saveProvider({ ...s.suggested, model });
      toast.success(`${s.name} added`, model);
      onAdded();
    } catch (e) {
      toast.error(`Could not add ${s.name}`, errorMessage(e));
    }
  };
  return (
    <div className="card col-sm">
      <div className="row">
        <span className="strong">{s.name}</span>
        {s.version && <span className="badge outline">v{s.version}</span>}
        <span className="mono xs subtle">{s.baseUrl}</span>
        <span className="spacer" />
        {s.models.length > 0 && (
          <select className="select" value={model} onChange={(e) => setModel(e.target.value)} style={{ maxWidth: 260 }}>
            {s.models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.id}
                {m.details ? ` (${m.details})` : ''}
                {m.warning ? ' ⚠' : ''}
              </option>
            ))}
          </select>
        )}
        <button className="btn btn-sm btn-primary" onClick={add} disabled={!model}>
          <Plus /> Add
        </button>
      </div>
      {[...s.warnings, ...s.models.filter((m) => m.id === model && m.warning).map((m) => m.warning!)].map((w) => (
        <Callout key={w} kind="warning">
          {w}
        </Callout>
      ))}
    </div>
  );
}

const slug = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '') || 'provider';

function ProviderModal({ initial, existingIds, onClose, onSaved }: { initial?: ProviderConfig; existingIds: string[]; onClose(): void; onSaved(): void }) {
  const [p, setP] = useState<ProviderConfig>(
    initial ?? { id: 'ollama', name: 'Ollama', kind: 'ollama', baseUrl: 'http://127.0.0.1:11434', model: 'qwen3.5:9b', disableThinking: true },
  );
  const [key, setKey] = useState('');
  const [clearKey, setClearKey] = useState(false);
  const [models, setModels] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const local = isLoopback(p.baseUrl);
  const set = (patch: Partial<ProviderConfig>) => setP((x) => ({ ...x, ...patch }));

  const applyPreset = (label: string) => {
    const pr = PROVIDER_PRESETS.find((x) => x.label === label);
    if (!pr) return;
    let id = slug(pr.label);
    for (let i = 2; !initial && existingIds.includes(id); i++) id = `${slug(pr.label)}-${i}`;
    setP((x) => ({ ...x, id: initial ? x.id : id, name: pr.label, kind: pr.kind, baseUrl: pr.baseUrl, model: pr.model || x.model }));
    setModels(null);
  };

  const loadModels = async () => {
    setBusy(true);
    setError(null);
    try {
      setModels(await aiApi.testProvider({ provider: p, apiKey: key || undefined }));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await aiApi.saveProvider(p, clearKey ? '' : key ? key : undefined);
      onSaved();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={onClose}
      title={initial ? `Edit ${initial.name}` : 'Add a model provider'}
      footer={
        <>
          <span className="spacer" />
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={save} disabled={busy || !p.model.trim() || !p.baseUrl.trim()}>
            Save
          </button>
        </>
      }
    >
      <div className="col">
        <Field label="Preset">
          <select className="select" value="" onChange={(e) => applyPreset(e.target.value)}>
            <option value="">Choose a preset…</option>
            {PROVIDER_PRESETS.map((pr) => (
              <option key={pr.label} value={pr.label}>
                {pr.label} {pr.local ? '(local)' : '(cloud)'}
              </option>
            ))}
          </select>
        </Field>
        <div className="grid-2">
          <Field label="Name">
            <input className="input" value={p.name} onChange={(e) => set({ name: e.target.value, ...(initial ? {} : { id: slug(e.target.value) }) })} />
          </Field>
          <Field label="API style">
            <select className="select" value={p.kind} onChange={(e) => set({ kind: e.target.value as ProviderKind })}>
              {Object.entries(KIND_LABEL).map(([k, v]) => (
                <option key={k} value={k}>
                  {v}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <Field label="Base URL" hint={local ? 'Local endpoint: prompts stay on this machine.' : 'Cloud endpoint: prompts leave this machine (credential-like strings are masked).'}>
          <input className="input mono" value={p.baseUrl} onChange={(e) => set({ baseUrl: e.target.value })} />
        </Field>
        <Field label="Model">
          <div className="row">
            {models && models.length > 0 ? (
              <select className="select grow" value={p.model} onChange={(e) => set({ model: e.target.value })}>
                {!models.includes(p.model) && <option value={p.model}>{p.model || '—'}</option>}
                {models.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            ) : (
              <input className="input grow mono" value={p.model} onChange={(e) => set({ model: e.target.value })} placeholder="model id" />
            )}
            <button className="btn btn-sm" onClick={loadModels} disabled={busy}>
              {busy ? <Spinner /> : <RefreshCw />} Load models
            </button>
          </div>
        </Field>
        <Field label="API key" hint={p.hasApiKey ? 'A key is stored in the OS keychain. Leave empty to keep it.' : 'Stored in the OS keychain — never in a file.'}>
          <div className="row">
            <input className="input grow" type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder={p.hasApiKey ? '••••••••' : local ? 'usually not needed' : 'required'} autoComplete="off" />
            {p.hasApiKey && <Switch checked={clearKey} onChange={setClearKey} label={<span className="small">remove stored key</span>} />}
          </div>
        </Field>
        <Switch checked={p.disableThinking !== false} onChange={(v) => set({ disableThinking: v })} label={<span className="small">Turn off “thinking” for local reasoning models (faster, better JSON)</span>} />
        {!local && (
          <Switch
            checked={!!p.trustedLocal}
            onChange={(v) => set({ trustedLocal: v })}
            label={<span className="small">This is a machine I control on my network (treat as local for redaction)</span>}
          />
        )}
        {error && <Callout kind="danger">{error}</Callout>}
      </div>
    </Modal>
  );
}
