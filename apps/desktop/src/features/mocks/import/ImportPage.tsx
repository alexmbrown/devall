import { useState } from 'react';
import clsx from 'clsx';
import { Upload, Download, FileArchive, FileJson2, ClipboardPaste, Package, ArrowRight } from 'lucide-react';
import { useMappings, useWm, useWrite, qk } from '@/features/mocks/lib/queries';
import { BUILTIN_ID } from '@/features/mocks/store/target';
import { parseYamlText } from '@/features/mocks/lib/servers';
import { useNav } from '@/features/mocks/store/nav';
import { importFile, importTextAsync, buildZip, SNAPSHOT_KIND, type ImportResult } from '@/features/mocks/lib/importers';
import { downloadBlob, downloadJson } from '@/features/mocks/lib/download';
import { cleanForSave, stubId, urlOf, withFolder } from '@/features/mocks/lib/stub-utils';
import { MethodBadge, StatusCode, Checkbox, Field, Callout, Spinner, Segmented, formatBytes } from '@/features/mocks/components/ui';
import { toast } from '@/components/ui/toast';

export default function ImportPage() {
  const { client, server } = useWm();
  const mappings = useMappings();
  const write = useWrite();
  const builtin = server.id === BUILTIN_ID;
  const go = useNav((s) => s.go);
  const [results, setResults] = useState<ImportResult[]>([]);
  const [paste, setPaste] = useState('');
  const [checked, setChecked] = useState<Set<number>>(new Set());
  const [policy, setPolicy] = useState<'OVERWRITE' | 'IGNORE'>('OVERWRITE');
  const [deleteOthers, setDeleteOthers] = useState(false);
  const [folder, setFolder] = useState('');
  const [over, setOver] = useState(false);
  const [busy, setBusy] = useState(false);

  const stubs = results.flatMap((r) => r.stubs);
  const files = Object.assign({}, ...results.map((r) => r.files)) as Record<string, Uint8Array>;
  const projects = results.flatMap((r) => r.projects ?? []);
  const messageStubs = results.flatMap((r) => r.messageStubs ?? []);

  const add = (r: ImportResult[]) => {
    const next = [...results, ...r];
    setResults(next);
    setChecked(new Set(next.flatMap((x) => x.stubs).map((_, i) => i)));
  };

  const onFiles = async (list: File[]) => {
    const out: ImportResult[] = [];
    for (const f of list) {
      try {
        out.push(await importFile(f, parseYamlText));
      } catch (e) {
        toast.error(`Could not import ${f.name}`, e);
      }
    }
    add(out);
  };

  const doImport = async () => {
    const chosen = stubs
      .filter((_, i) => checked.has(i))
      .map((s) => (folder ? withFolder(s, folder) : s))
      // Imported into the built-in engine = saved as project files.
      .map((s) => cleanForSave(builtin && s.persistent === undefined ? { ...s, persistent: true } : s));
    setBusy(true);
    try {
      await write(
        async (c) => {
          for (const [p, b] of Object.entries(files)) await c.putFile(p, b);
          if (chosen.length) await c.importMappings(chosen, policy, deleteOthers);
        },
        {
          label: 'Import',
          success: `Imported ${chosen.length} stub${chosen.length === 1 ? '' : 's'}${Object.keys(files).length ? ` and ${Object.keys(files).length} files` : ''}`,
          invalidate: (s) => [qk.mappings(s), qk.files(s), qk.scenarios(s)],
          confirm: deleteOthers ? { title: 'Replace all stubs?', message: '“Delete stubs not in this import” is on — every other stub on the server is removed.', danger: true, confirmText: 'Import & replace' } : undefined,
        },
      );
      setResults([]);
      setChecked(new Set());
    } finally {
      setBusy(false);
    }
  };

  const exportZip = async () => {
    if (!client) return;
    setBusy(true);
    try {
      const all = (await client.mappings()).mappings;
      const names = await client.files();
      const fileMap: Record<string, Uint8Array> = {};
      for (const n of names) fileMap[n] = await client.fileBytes(n);
      downloadBlob(`wiremock-${safe(server?.name)}-${date()}.zip`, new Blob([buildZip(all.map(cleanForSave), fileMap).slice().buffer], { type: 'application/zip' }));
      toast.success(`Exported ${all.length} stubs and ${names.length} files`);
    } catch (e) {
      toast.error('Export failed', e);
    } finally {
      setBusy(false);
    }
  };

  const exportSnapshot = async () => {
    if (!client) return;
    setBusy(true);
    try {
      const [m, names, settings, scenarios, version] = await Promise.all([
        client.mappings(),
        client.files(),
        client.getSettings(),
        client.scenarios().catch(() => ({ scenarios: [] })),
        client.version().catch(() => undefined),
      ]);
      const filesB64: Record<string, string> = {};
      for (const n of names) {
        const b = await client.fileBytes(n);
        let s = '';
        for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode(...b.subarray(i, i + 0x8000));
        filesB64[n] = btoa(s);
      }
      downloadJson(`wiremock-${safe(server?.name)}-snapshot-${date()}.json`, {
        kind: SNAPSHOT_KIND,
        version: 1,
        exportedAt: new Date().toISOString(),
        server: { name: server?.name, url: server?.url, wiremockVersion: version },
        settings,
        scenarios: scenarios.scenarios.map((s) => ({ name: s.name, state: s.state })),
        mappings: m.mappings.map(cleanForSave),
        files: filesB64,
      });
    } catch (e) {
      toast.error('Export failed', e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page">
      <div className="page-header">
        <h1>Import / export</h1>
        <span className="subtle small">{builtin ? 'Target: this project — imported stubs are saved to mocks/mappings/' : `Target: ${server.name}`}</span>
      </div>
      <div className="page-body">
        <div className="grid-2" style={{ gridTemplateColumns: '1.4fr 1fr', alignItems: 'start' }}>
          <div className="col-lg">
            <div className="panel">
              <div className="panel-header">
                <Upload size={15} className="subtle" />
                <h3 className="grow">Import</h3>
              </div>
              <div className="panel-body col">
                <label
                  className={clsx('drop-zone', over && 'over')}
                  onDragOver={(e) => {
                    e.preventDefault();
                    setOver(true);
                  }}
                  onDragLeave={() => setOver(false)}
                  onDrop={(e) => {
                    e.preventDefault();
                    setOver(false);
                    onFiles([...e.dataTransfer.files]);
                  }}
                >
                  <input type="file" multiple hidden onChange={(e) => onFiles([...(e.target.files ?? [])])} accept=".json,.zip,.har,.yaml,.yml,.txt,.sh" />
                  <Upload size={22} />
                  <div className="strong mt-2">Drop files or click to choose</div>
                  <div className="xs mt-2">
                    WireMock stub JSON · <code className="code-inline">{'{mappings:[…]}'}</code> · ZIP of <code className="code-inline">mappings/</code> + <code className="code-inline">__files/</code> · HAR from browser DevTools · OpenAPI 3 / Swagger 2 (JSON or YAML) · Postman collection · WireMock UI snapshot
                  </div>
                </label>
                <Field label="…or paste JSON, YAML or one or more cURL commands">
                  <textarea className="textarea mono" rows={4} value={paste} onChange={(e) => setPaste(e.target.value)} placeholder={"curl -X POST https://api.example.com/orders -H 'Content-Type: application/json' -d '{\"sku\":\"A1\"}'"} />
                </Field>
                <div className="row">
                  <button
                    className="btn btn-sm"
                    disabled={!paste.trim()}
                    onClick={() => {
                      try {
                        importTextAsync(paste, 'pasted text', parseYamlText)
                          .then((r) => {
                            add([r]);
                            setPaste('');
                          })
                          .catch((e) => toast.error('Could not parse', e));
                      } catch (e) {
                        toast.error('Could not parse', e);
                      }
                    }}
                  >
                    <ClipboardPaste /> Parse
                  </button>
                  {results.length > 0 && (
                    <button className="btn btn-sm btn-ghost" onClick={() => (setResults([]), setChecked(new Set()))}>
                      Clear
                    </button>
                  )}
                </div>
              </div>
            </div>

            {results.length > 0 && (
              <div className="panel">
                <div className="panel-header">
                  <h3 className="grow">Preview</h3>
                  <span className="subtle xs">{results.map((r) => `${r.source} (${r.kind})`).join(', ')}</span>
                </div>
                <div className="panel-body col">
                  {results.flatMap((r) => r.warnings).slice(0, 8).map((w, i) => (
                    <div key={i} className="text-warning xs">
                      {w}
                    </div>
                  ))}
                  {projects.length > 0 && (
                    <Callout kind="info">
                      This file contains WireMock UI projects (browser server connections). In /dev/all remote servers live in <code className="code-inline">mocks/servers.yaml</code> —
                      add them under Servers.
                    </Callout>
                  )}
                  {stubs.length > 0 && (
                    <>
                      <div className="row">
                        <Checkbox checked={checked.size === stubs.length} indeterminate={checked.size > 0 && checked.size < stubs.length} onChange={(v) => setChecked(v ? new Set(stubs.map((_, i) => i)) : new Set())} />
                        <span className="small grow">
                          {checked.size} of {stubs.length} stubs selected
                          {Object.keys(files).length > 0 && ` · ${Object.keys(files).length} body files (${formatBytes(Object.values(files).reduce((a, b) => a + b.length, 0))})`}
                        </span>
                      </div>
                      <div className="list" style={{ maxHeight: 360, border: '1px solid var(--border)', borderRadius: 'var(--radius)' }}>
                        {stubs.map((s, i) => {
                          const exists = s.id && mappings.data?.some((m) => stubId(m) === s.id);
                          return (
                            <div key={i} className="list-item" onClick={() => {
                              const n = new Set(checked);
                              if (n.has(i)) n.delete(i);
                              else n.add(i);
                              setChecked(n);
                            }}>
                              <Checkbox checked={checked.has(i)} onChange={() => {}} />
                              <MethodBadge method={s.request?.method} />
                              <span className="truncate grow">{s.name || urlOf(s.request ?? {})}</span>
                              {s.name && <span className="mono xs subtle truncate" style={{ maxWidth: 220 }}>{urlOf(s.request ?? {})}</span>}
                              <StatusCode status={s.response?.status ?? 200} />
                              {exists && <span className="badge warning">exists</span>}
                            </div>
                          );
                        })}
                      </div>
                      <div className="grid-2">
                        <Field label="Stubs with an existing id">
                          <Segmented value={policy} onChange={setPolicy} options={[{ value: 'OVERWRITE', label: 'Overwrite' }, { value: 'IGNORE', label: 'Keep existing' }]} />
                        </Field>
                        <Field label="Put into folder (optional)">
                          <input className="input sm" value={folder} onChange={(e) => setFolder(e.target.value)} placeholder="keep original folders" />
                        </Field>
                      </div>
                      <Checkbox checked={deleteOthers} onChange={setDeleteOthers} label={<span className={deleteOthers ? 'text-danger' : ''}>Delete stubs on the server that are not in this import (make the server match the file)</span>} />
                    </>
                  )}
                  {messageStubs.length > 0 && (
                    <Callout kind="warning">
                      {messageStubs.length} WireMock 4 message stub{messageStubs.length === 1 ? '' : 's'} (WebSocket / SSE) skipped: this UI imports HTTP stubs only.
                    </Callout>
                  )}
                  {(stubs.length > 0 || Object.keys(files).length > 0) && (
                    <div className="row">
                      <span className="spacer" />
                      <button className="btn btn-primary" disabled={busy || (!checked.size && !Object.keys(files).length)} onClick={doImport}>
                        {busy ? <Spinner /> : <Upload />} Import to {builtin ? 'this project' : server.name}
                      </button>
                    </div>
                  )}
                </div>
              </div>
            )}
          </div>

          <div className="col-lg">
            <div className="panel">
              <div className="panel-header">
                <Download size={15} className="subtle" />
                <h3 className="grow">Export {server?.name}</h3>
              </div>
              <div className="panel-body col">
                <ExportRow
                  icon={<FileJson2 />}
                  title="Stubs JSON"
                  desc="{mappings:[…]} — re-import here or POST to /__admin/mappings/import."
                  onClick={() => downloadJson(`wiremock-${safe(server?.name)}-${date()}.json`, { mappings: (mappings.data ?? []).map(cleanForSave) })}
                />
                <ExportRow icon={<FileArchive />} title="ZIP (WireMock root dir)" desc="mappings/<folder>/*.json + __files/ — commit to git or start WireMock with --root-dir." onClick={exportZip} busy={busy} />
                <ExportRow icon={<Package />} title="Full snapshot" desc="Stubs, body files, global settings and scenario states in one JSON file — for backups or cloning a server." onClick={exportSnapshot} busy={busy} />
                <div className="divider" />
                <button className="btn btn-sm" onClick={() => go('compare')}>
                  Copy stubs to another server <ArrowRight />
                </button>
                <button className="btn btn-sm" onClick={() => go('servers')}>
                  Manage remote servers <ArrowRight />
                </button>
              </div>
            </div>
            <Callout>
              <p className="strong">No proxy? Record with your browser.</p>
              <p className="small">
                Open DevTools → Network on the real app, use it, then right-click → <i>Save all as HAR</i>. Dropping the .har here creates one stub per unique request with the real
                responses (static assets are skipped).
              </p>
            </Callout>
          </div>
        </div>
      </div>
    </div>
  );
}

function ExportRow({ icon, title, desc, onClick, busy }: { icon: React.ReactNode; title: string; desc: string; onClick(): void; busy?: boolean }) {
  return (
    <div className="row items-start">
      <span className="subtle" style={{ marginTop: 2 }}>
        {icon}
      </span>
      <div className="grow">
        <div className="strong small">{title}</div>
        <div className="subtle xs">{desc}</div>
      </div>
      <button className="btn btn-sm" onClick={onClick} disabled={busy}>
        <Download /> Download
      </button>
    </div>
  );
}

const safe = (s?: string) => (s ?? 'server').replace(/[^\w.-]+/g, '-').toLowerCase();
const date = () => new Date().toISOString().slice(0, 10);
