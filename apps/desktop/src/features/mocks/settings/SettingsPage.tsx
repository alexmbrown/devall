import { useEffect, useState } from 'react';
import { SlidersHorizontal, Save, RotateCcw, Trash2, Power, Eraser, HardDrive, Timer, Terminal } from 'lucide-react';
import type { DelayDistribution, GlobalSettings } from '@/features/mocks/lib/types';
import { useSettings, useWrite, useWm, qk } from '@/features/mocks/lib/queries';
import { Field, Switch, Segmented, Callout, Spinner, CopyButton, Checkbox } from '@/features/mocks/components/ui';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { prettyJson, tryParseJson } from '@/features/mocks/lib/stub-utils';
import { startupCommands, type StartupOpts } from '@/features/mocks/settings/startup';
import { usePrefs } from '@/features/mocks/store/prefs';
import { useMockTarget } from '@/features/mocks/store/target';

type DelayKind = 'none' | 'fixed' | 'uniform' | 'lognormal';

const PRESETS: { label: string; s: Partial<GlobalSettings> }[] = [
  { label: 'No delay', s: { fixedDelay: undefined, delayDistribution: undefined } },
  { label: 'Fast LAN (5–20 ms)', s: { fixedDelay: undefined, delayDistribution: { type: 'uniform', lower: 5, upper: 20 } } },
  { label: 'Typical API (~120 ms)', s: { fixedDelay: undefined, delayDistribution: { type: 'lognormal', median: 120, sigma: 0.3 } } },
  { label: 'Slow 3G (~800 ms)', s: { fixedDelay: undefined, delayDistribution: { type: 'lognormal', median: 800, sigma: 0.5 } } },
  { label: 'Timeout test (30 s)', s: { fixedDelay: 30000, delayDistribution: undefined } },
];

export default function SettingsPage() {
  const settings = useSettings();
  const write = useWrite();
  const { server } = useWm();
  const builtin = server.kind === 'builtin';
  const [draft, setDraft] = useState<GlobalSettings | null>(null);
  const [extText, setExtText] = useState('');
  const s = draft ?? settings.data ?? {};
  useEffect(() => {
    if (settings.data && !draft) setExtText(settings.data.extended ? prettyJson(settings.data.extended) : '');
  }, [settings.data, draft]);
  const kind: DelayKind = s.fixedDelay ? 'fixed' : s.delayDistribution?.type === 'uniform' ? 'uniform' : s.delayDistribution?.type === 'lognormal' ? 'lognormal' : 'none';
  const dd = s.delayDistribution as DelayDistribution | undefined;
  const set = (patch: Partial<GlobalSettings>) => setDraft({ ...s, ...patch });
  const inv = (sid: string) => [qk.settings(sid)];

  const apply = async (next: GlobalSettings) => {
    // Keep undefined keys: updateSettings() merges with the server copy and drops them.
    await write((c) => c.updateSettings(next).then(() => c.getSettings()), { label: 'Update settings', success: 'Global settings updated', invalidate: inv });
    setDraft(null);
  };

  return (
    <div className="page">
      <div className="page-header">
        <SlidersHorizontal size={16} className="subtle" />
        <h1>Server settings</h1>
        <span className="subtle small">
          Global settings for <b>{server?.name}</b> — they affect every stub on this server.
        </span>
        <div className="spacer" />
        {draft && (
          <>
            <button className="btn btn-sm btn-ghost" onClick={() => setDraft(null)}>
              Discard
            </button>
            <button className="btn btn-sm btn-primary" onClick={() => apply(draft)}>
              <Save /> Apply
            </button>
          </>
        )}
      </div>
      <div className="page-body">
        {settings.isPending ? (
          <Spinner />
        ) : (
          <div className="col-lg" style={{ maxWidth: 980 }}>
            <div className="panel">
              <div className="panel-header">
                <Timer size={15} className="subtle" />
                <h3 className="grow">Global latency</h3>
                <span className="subtle xs">Added to every response, on top of per-stub delays</span>
              </div>
              <div className="panel-body col">
                <div className="row wrap">
                  {PRESETS.map((p) => (
                    <button key={p.label} className="btn btn-sm" onClick={() => apply({ ...s, ...p.s })}>
                      {p.label}
                    </button>
                  ))}
                </div>
                <Segmented<DelayKind>
                  value={kind}
                  onChange={(k) =>
                    set(
                      k === 'none'
                        ? { fixedDelay: undefined, delayDistribution: undefined }
                        : k === 'fixed'
                          ? { fixedDelay: 500, delayDistribution: undefined }
                          : k === 'uniform'
                            ? { fixedDelay: undefined, delayDistribution: { type: 'uniform', lower: 100, upper: 500 } }
                            : { fixedDelay: undefined, delayDistribution: { type: 'lognormal', median: 200, sigma: 0.4 } },
                    )
                  }
                  options={[
                    { value: 'none', label: 'None' },
                    { value: 'fixed', label: 'Fixed' },
                    { value: 'uniform', label: 'Uniform random' },
                    { value: 'lognormal', label: 'Log-normal' },
                  ]}
                />
                {kind === 'fixed' && (
                  <div className="row-sm">
                    <input className="input sm" type="number" style={{ width: 110 }} value={s.fixedDelay ?? 0} onChange={(e) => set({ fixedDelay: Number(e.target.value) || undefined })} />
                    <span className="subtle xs">ms</span>
                  </div>
                )}
                {kind === 'uniform' && dd?.type === 'uniform' && (
                  <div className="row-sm">
                    <input className="input sm" type="number" style={{ width: 90 }} value={dd.lower} onChange={(e) => set({ delayDistribution: { ...dd, lower: Number(e.target.value) } })} />
                    <span className="subtle xs">to</span>
                    <input className="input sm" type="number" style={{ width: 90 }} value={dd.upper} onChange={(e) => set({ delayDistribution: { ...dd, upper: Number(e.target.value) } })} />
                    <span className="subtle xs">ms</span>
                  </div>
                )}
                {kind === 'lognormal' && dd?.type === 'lognormal' && (
                  <div className="row-sm">
                    <span className="subtle xs">median</span>
                    <input className="input sm" type="number" style={{ width: 90 }} value={dd.median} onChange={(e) => set({ delayDistribution: { ...dd, median: Number(e.target.value) } })} />
                    <span className="subtle xs">ms · sigma</span>
                    <input className="input sm" type="number" step="0.05" style={{ width: 80 }} value={dd.sigma} onChange={(e) => set({ delayDistribution: { ...dd, sigma: Number(e.target.value) } })} />
                  </div>
                )}
              </div>
            </div>

            <div className="panel">
              <div className="panel-header">
                <h3 className="grow">Proxying & extensions</h3>
              </div>
              <div className="panel-body col">
                <Switch
                  checked={s.proxyPassThrough !== false}
                  onChange={(v) => apply({ ...s, proxyPassThrough: v })}
                  label="Proxy pass-through — let proxy stubs forward to upstream (turn off to freeze traffic to real backends)"
                />
                <Field label="Extended settings (JSON, read by extensions)">
                  <div className="editor-shell">
                    <CodeEditor
                      value={extText}
                      autoHeight={{ min: 60, max: 260 }}
                      minimal
                      onChange={(t) => {
                        setExtText(t);
                        const r = tryParseJson<Record<string, unknown>>(t);
                        if (!t.trim()) set({ extended: undefined });
                        else if (r.ok) set({ extended: r.value });
                      }}
                    />
                  </div>
                </Field>
                <div className="subtle xs">WireMock replaces the whole settings object on update; this page always sends the merged result so nothing is lost.</div>
              </div>
            </div>

            <div className="panel" style={{ borderColor: 'var(--danger-border)' }}>
              <div className="panel-header">
                <h3 className="grow text-danger">Server actions</h3>
              </div>
              <div className="panel-body col">
                <ActionRow
                  icon={<HardDrive />}
                  title="Persist all stubs to disk"
                  desc={builtin ? 'Writes every in-memory stub to mocks/mappings/ in this project.' : "POST /mappings/save — writes every stub to the server's mappings folder and marks them all persistent."}
                  btn="Persist all"
                  run={() => write((c) => c.saveMappings(), { label: 'Save mappings', success: 'All stubs persisted' })}
                />
                <ActionRow
                  icon={<Eraser />}
                  title="Clear request journal"
                  desc="DELETE /requests — removes logged requests; stubs are untouched."
                  btn="Clear"
                  run={() => write((c) => c.clearRequests(), { label: 'Clear journal', success: 'Journal cleared' })}
                />
                <ActionRow
                  icon={<RotateCcw />}
                  title="Reset scenarios"
                  desc="Every scenario returns to Started."
                  btn="Reset"
                  run={() => write((c) => c.resetScenarios(), { label: 'Reset scenarios', success: 'Scenarios reset' })}
                />
                <ActionRow
                  icon={<RotateCcw />}
                  title="Reset to mappings on disk"
                  desc={builtin ? 'Drops in-memory stubs, clears the journal and scenarios, and reloads mocks/mappings/ from disk.' : 'POST /reset — drops non-persistent stubs, clears the journal and scenarios, reloads files from disk.'}
                  btn="Reset server"
                  danger
                  run={() => write((c) => c.resetAll(), { label: 'Reset', success: 'Server reset', confirm: { title: 'Reset the server?', message: 'Non-persistent stubs are lost.', danger: true, confirmText: 'Reset' } })}
                />
                <ActionRow
                  icon={<Trash2 />}
                  title="Delete all stubs"
                  desc={builtin ? 'Removes every stub and moves their files in mocks/mappings/ to the OS trash.' : 'DELETE /mappings — removes every stub AND deletes their files from the mappings folder. Consider exporting first.'}
                  btn="Delete all"
                  danger
                  run={() =>
                    write((c) => c.deleteAllMappings(), {
                      label: 'Delete all stubs',
                      success: 'All stubs deleted',
                      confirm: { title: 'Delete ALL stubs?', message: 'This also deletes mapping files on the server. Export first if you might need them.', danger: true, confirmText: 'Delete everything' },
                    })
                  }
                />
                {builtin ? (
                  <ActionRow icon={<Power />} title="Stop the mock server" desc="Stops the built-in engine; stub files stay in mocks/mappings. Start it again from the bar above." btn="Stop" run={() => useMockTarget.getState().stop()} />
                ) : (
                  <ActionRow
                    icon={<Power />}
                    title="Shut down WireMock"
                    desc="POST /shutdown — the process exits. You'll need to restart it outside this app."
                    btn="Shut down"
                    danger
                    run={() => write((c) => c.shutdown(), { label: 'Shutdown', success: 'Shutdown requested', confirm: { title: 'Shut down this WireMock server?', danger: true, confirmText: 'Shut down' } })}
                  />
                )}
              </div>
            </div>

            <StartupBuilder builtin={builtin} />
            <UiPrefs />
          </div>
        )}
      </div>
    </div>
  );
}

function ActionRow({ icon, title, desc, btn, run, danger }: { icon: React.ReactNode; title: string; desc: string; btn: string; run(): void; danger?: boolean }) {
  return (
    <div className="row">
      <span className="subtle">{icon}</span>
      <div className="grow">
        <div className="strong small">{title}</div>
        <div className="subtle xs">{desc}</div>
      </div>
      <button className={danger ? 'btn btn-sm btn-danger-ghost' : 'btn btn-sm'} onClick={run}>
        {btn}
      </button>
    </div>
  );
}

function StartupBuilder({ builtin }: { builtin: boolean }) {
  // The project's mocks/ folder is a WireMock root dir: CI can serve the same stubs with stock WireMock.
  const [o, setO] = useState<StartupOpts>({ port: 8080, cors: true, templating: true, verbose: false, rootDir: builtin ? './mocks' : undefined });
  const cmds = startupCommands(o);
  return (
    <div className="panel">
      <div className="panel-header">
        <Terminal size={15} className="subtle" />
        <h3 className="grow">Startup command builder</h3>
        <span className="subtle xs">{builtin ? 'Serve this project’s mocks/ with stock WireMock, e.g. in CI' : "Command-line flags can't be changed at runtime"}</span>
      </div>
      <div className="panel-body col">
        <div className="row wrap">
          <Field label="Port">
            <input className="input sm" type="number" style={{ width: 90 }} value={o.port} onChange={(e) => setO({ ...o, port: Number(e.target.value) })} />
          </Field>
          <Field label="Admin basic auth">
            <input className="input sm" style={{ width: 140 }} placeholder="user:pass" value={o.basicAuth ?? ''} onChange={(e) => setO({ ...o, basicAuth: e.target.value || undefined })} />
          </Field>
          <Field label="Proxy all to">
            <input className="input sm" style={{ width: 200 }} placeholder="https://api.example.com" value={o.proxyAll ?? ''} onChange={(e) => setO({ ...o, proxyAll: e.target.value || undefined })} />
          </Field>
          <Field label="HTTPS port">
            <input className="input sm" type="number" style={{ width: 90 }} value={o.httpsPort ?? ''} onChange={(e) => setO({ ...o, httpsPort: Number(e.target.value) || undefined })} />
          </Field>
          <Field label="Journal max">
            <input className="input sm" type="number" style={{ width: 90 }} placeholder="∞" value={o.journalMax ?? ''} onChange={(e) => setO({ ...o, journalMax: Number(e.target.value) || undefined })} />
          </Field>
        </div>
        <div className="row wrap">
          <Checkbox checked={o.cors !== false} onChange={(cors) => setO({ ...o, cors })} label="--enable-stub-cors (browser apps can call stubs)" />
          <Checkbox checked={!!o.templating} onChange={(templating) => setO({ ...o, templating })} label="--global-response-templating" />
          <Checkbox checked={!!o.browserProxying} onChange={(browserProxying) => setO({ ...o, browserProxying })} label="--enable-browser-proxying" />
          <Checkbox checked={!!o.verbose} onChange={(verbose) => setO({ ...o, verbose })} label="--verbose" />
        </div>
        {o.basicAuth && <Callout kind="info">The password can't contain a colon. Add the same credentials to this server's connection settings.</Callout>}
        {cmds.map((c) => (
          <div key={c.label} className="col-sm">
            <div className="row between">
              <span className="label">{c.label}</span>
              <CopyButton text={c.cmd} />
            </div>
            <pre className="code">{c.cmd}</pre>
          </div>
        ))}
      </div>
    </div>
  );
}

function UiPrefs() {
  const p = usePrefs();
  return (
    <div className="panel">
      <div className="panel-header">
        <h3 className="grow">Mock UI preferences</h3>
        <span className="subtle xs">Stored on this machine · theme, density and word wrap are in the app settings</span>
      </div>
      <div className="panel-body col">
        <div className="row wrap">
          <Field label="Default stub editor">
            <Segmented value={p.editorMode} onChange={(v) => p.set('editorMode', v)} options={[{ value: 'form', label: 'Form' }, { value: 'json', label: 'JSON' }]} />
          </Field>
          <Field label="Live journal polling">
            <select className="select sm" style={{ width: 140 }} value={p.pollMs} onChange={(e) => p.set('pollMs', Number(e.target.value))}>
              {[0, 1000, 2000, 3000, 5000, 10000].map((v) => (
                <option key={v} value={v}>
                  {v ? `every ${v / 1000}s` : 'off'}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <Switch checked={p.showFolders} onChange={(v) => p.set('showFolders', v)} label="Show stub folders in the list" />
      </div>
    </div>
  );
}
