import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { ArrowDown, ArrowUp, Copy, Download, FolderOpen, Plus, Save, Trash2, Upload, Wand2, X } from 'lucide-react';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { Modal } from '@/components/ui/Modal';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Callout, Checkbox, Empty, Field, Switch } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import { proxyApi, type NameValue, type Rule, type RuleAction, type RuleActionType } from '@/lib/api/proxy';
import { openapiApi } from '@/lib/api/openapi';
import { useWorkspace } from '@/store/workspace';
import { useInspector } from './store';

const ACTION_LABEL: Record<RuleActionType, string> = {
  breakpoint: 'Breakpoint',
  mapLocal: 'Map Local',
  mapRemote: 'Map Remote',
  mockResponse: 'Mock response',
  setHeader: 'Set header',
  removeHeader: 'Remove header',
  throttle: 'Throttle',
  block: 'Block',
  validateSpec: 'Validate against OpenAPI',
};

function newAction(type: RuleActionType): RuleAction {
  switch (type) {
    case 'breakpoint':
      return { type, request: true, response: false };
    case 'mapLocal':
      return { type, status: 200, body: '{}' };
    case 'mapRemote':
      return { type, scheme: 'http', host: 'localhost', port: 3000 };
    case 'mockResponse':
      return { type, status: 200, headers: [{ name: 'content-type', value: 'application/json' }], body: '{\n  "ok": true\n}' };
    case 'setHeader':
      return { type, target: 'request', name: 'x-debug', value: '1' };
    case 'removeHeader':
      return { type, target: 'response', name: 'cache-control' };
    case 'throttle':
      return { type, latencyMs: 500, kbps: 50 };
    case 'block':
      return { type, mode: 'forbidden' };
    case 'validateSpec':
      return { type, spec: '', mode: 'flag' };
  }
}

export function newRule(partial: Partial<Rule> = {}, action: RuleActionType = 'breakpoint'): Rule {
  return { id: '', name: partial.name ?? `${ACTION_LABEL[action]} rule`, match: partial.match ?? {}, actions: partial.actions ?? [newAction(action)] };
}

function summarize(r: Rule): string {
  const m = r.match ?? {};
  const parts = [m.methods?.length ? m.methods.join(',') : '', m.host ?? '', m.path ?? (m.pathRegex ? `/${m.pathRegex}/` : ''), m.header ? `[${m.header.name}${m.header.value ? `=${m.header.value}` : ''}]` : ''].filter(Boolean);
  return parts.length ? parts.join(' ') : 'every request';
}

const headersText = (h?: NameValue[]) => (h ?? []).map((x) => `${x.name}: ${x.value}`).join('\n');
const parseHeaders = (t: string): NameValue[] =>
  t
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean)
    .map((l) => {
      const i = l.indexOf(':');
      return i > 0 ? { name: l.slice(0, i).trim(), value: l.slice(i + 1).trim() } : { name: l, value: '' };
    });

const num = (v: string): number | undefined => (v.trim() === '' || Number.isNaN(Number(v)) ? undefined : Number(v));
const str = (v: string): string | undefined => (v.trim() === '' ? undefined : v);

function ActionEditor({ a, onChange }: { a: RuleAction; onChange(a: RuleAction): void }) {
  switch (a.type) {
    case 'breakpoint':
      return (
        <div className="row">
          <Checkbox checked={a.request !== false} onChange={(v) => onChange({ ...a, request: v })} label="Pause requests" />
          <Checkbox checked={!!a.response} onChange={(v) => onChange({ ...a, response: v })} label="Pause responses" />
        </div>
      );
    case 'mapLocal':
      return (
        <div className="col-sm">
          <Field label="File or folder (absolute path; a folder is joined with the request path)">
            <div className="row-sm">
              <input className="input sm mono grow" value={a.file ?? ''} placeholder="/path/to/response.json" onChange={(e) => onChange({ ...a, file: str(e.target.value) })} />
              <button
                className="btn btn-sm"
                onClick={async () => {
                  const p = await openDialog({ multiple: false, directory: false });
                  if (typeof p === 'string') onChange({ ...a, file: p });
                }}
              >
                <FolderOpen /> Browse
              </button>
            </div>
          </Field>
          <Field label="…or inline body (used when no file is set)">
            <textarea className="textarea mono" rows={4} value={a.body ?? ''} onChange={(e) => onChange({ ...a, body: e.target.value === '' ? undefined : e.target.value })} />
          </Field>
          <div className="grid-3">
            <Field label="Status">
              <input className="input sm" value={a.status ?? 200} onChange={(e) => onChange({ ...a, status: num(e.target.value) })} />
            </Field>
            <Field label="Content type (auto when empty)">
              <input className="input sm mono" value={a.contentType ?? ''} onChange={(e) => onChange({ ...a, contentType: str(e.target.value) })} />
            </Field>
          </div>
          <Field label="Extra headers (Name: value per line)">
            <textarea className="textarea mono" rows={2} value={headersText(a.headers)} onChange={(e) => onChange({ ...a, headers: parseHeaders(e.target.value) })} />
          </Field>
        </div>
      );
    case 'mapRemote':
      return (
        <div className="col-sm">
          <div className="grid-3">
            <Field label="Scheme">
              <select className="select sm" value={a.scheme ?? ''} onChange={(e) => onChange({ ...a, scheme: str(e.target.value) })}>
                <option value="">keep</option>
                <option value="http">http</option>
                <option value="https">https</option>
              </select>
            </Field>
            <Field label="Host">
              <input className="input sm mono" value={a.host ?? ''} placeholder="keep" onChange={(e) => onChange({ ...a, host: str(e.target.value) })} />
            </Field>
            <Field label="Port">
              <input className="input sm" value={a.port ?? ''} placeholder="keep" onChange={(e) => onChange({ ...a, port: num(e.target.value) })} />
            </Field>
          </div>
          <div className="grid-2">
            <Field label="Replace path prefix">
              <input className="input sm mono" value={a.pathFrom ?? ''} placeholder="/v1" onChange={(e) => onChange({ ...a, pathFrom: str(e.target.value) })} />
            </Field>
            <Field label="…with">
              <input className="input sm mono" value={a.pathTo ?? ''} placeholder="/api/v1" onChange={(e) => onChange({ ...a, pathTo: str(e.target.value) })} />
            </Field>
          </div>
          <Checkbox checked={!!a.preserveHost} onChange={(v) => onChange({ ...a, preserveHost: v || undefined })} label="Keep the original Host header" />
        </div>
      );
    case 'mockResponse':
      return (
        <div className="col-sm">
          <div className="grid-3">
            <Field label="Status">
              <input className="input sm" value={a.status ?? 200} onChange={(e) => onChange({ ...a, status: num(e.target.value) })} />
            </Field>
            <Field label="Delay (ms)">
              <input className="input sm" value={a.delayMs ?? ''} placeholder="0" onChange={(e) => onChange({ ...a, delayMs: num(e.target.value) })} />
            </Field>
          </div>
          <Field label="Headers (Name: value per line)">
            <textarea className="textarea mono" rows={2} value={headersText(a.headers)} onChange={(e) => onChange({ ...a, headers: parseHeaders(e.target.value) })} />
          </Field>
          <Field label="Body">
            <textarea className="textarea mono" rows={6} value={a.body ?? ''} onChange={(e) => onChange({ ...a, body: e.target.value })} />
          </Field>
        </div>
      );
    case 'setHeader':
    case 'removeHeader':
      return (
        <div className="grid-3">
          <Field label="On">
            <select className="select sm" value={a.target ?? 'request'} onChange={(e) => onChange({ ...a, target: e.target.value as 'request' | 'response' })}>
              <option value="request">request</option>
              <option value="response">response</option>
            </select>
          </Field>
          <Field label="Header">
            <input className="input sm mono" value={a.name} onChange={(e) => onChange({ ...a, name: e.target.value })} />
          </Field>
          {a.type === 'setHeader' && (
            <Field label="Value">
              <input className="input sm mono" value={a.value ?? ''} onChange={(e) => onChange({ ...a, value: e.target.value })} />
            </Field>
          )}
        </div>
      );
    case 'throttle':
      return (
        <div className="grid-3">
          <Field label="Added latency (ms)">
            <input className="input sm" value={a.latencyMs ?? ''} placeholder="0" onChange={(e) => onChange({ ...a, latencyMs: num(e.target.value) })} />
          </Field>
          <Field label="Download limit (KB/s)">
            <input className="input sm" value={a.kbps ?? ''} placeholder="unlimited" onChange={(e) => onChange({ ...a, kbps: num(e.target.value) })} />
          </Field>
        </div>
      );
    case 'block':
      return (
        <Field label="How">
          <select className="select sm" value={a.mode ?? 'forbidden'} onChange={(e) => onChange({ ...a, mode: e.target.value as 'forbidden' | 'close' })}>
            <option value="forbidden">Answer 403 Forbidden</option>
            <option value="close">Close the connection</option>
          </select>
        </Field>
      );
    case 'validateSpec':
      return <ValidateSpecEditor a={a} onChange={onChange} />;
  }
}

/** Spec picker (project specs, or any file) + flag / block mode. */
function ValidateSpecEditor({ a, onChange }: { a: Extract<RuleAction, { type: 'validateSpec' }>; onChange(a: RuleAction): void }) {
  const root = useWorkspace((s) => s.project?.root);
  const [specs, setSpecs] = useState<string[]>([]);
  useEffect(() => {
    if (root) openapiApi.specs(root).then(setSpecs, () => setSpecs([]));
  }, [root]);
  const sep = root?.includes('\\') ? '\\' : '/';
  const abs = (rel: string) => (root ? `${root.replace(/[\\/]+$/, '')}${sep}${rel.split('/').join(sep)}` : rel);
  const browse = async () => {
    const f = await openDialog({ multiple: false, filters: [{ name: 'OpenAPI', extensions: ['yaml', 'yml', 'json'] }] });
    if (typeof f === 'string') onChange({ ...a, spec: f });
  };
  return (
    <div className="col-sm">
      <Field label="Spec" hint="Loaded once when the rules are saved — save again after editing the spec.">
        <div className="row-sm">
          <select className="select sm grow" value={specs.some((s) => abs(s) === a.spec) ? a.spec : ''} onChange={(e) => e.target.value && onChange({ ...a, spec: e.target.value })}>
            <option value="">{a.spec ? a.spec : specs.length ? 'Pick a spec in this project…' : 'No OpenAPI files found in the project'}</option>
            {specs.map((s) => (
              <option key={s} value={abs(s)}>
                {s}
              </option>
            ))}
          </select>
          <button className="btn btn-sm" onClick={() => void browse()}>
            <FolderOpen /> Browse…
          </button>
        </div>
      </Field>
      <div className="grid-2">
        <Field label="On a violation">
          <select className="select sm" value={a.mode ?? 'flag'} onChange={(e) => onChange({ ...a, mode: e.target.value as 'flag' | 'block' })}>
            <option value="flag">Flag the exchange (badge + findings)</option>
            <option value="block">Block: 422 for bad requests, 502 for bad responses</option>
          </select>
        </Field>
        <Checkbox checked={!!a.allHosts} onChange={(v) => onChange({ ...a, allHosts: v || undefined })} label="Check every matching host (not only the spec's servers)" />
      </div>
    </div>
  );
}

function RuleEditor({ rule, onChange }: { rule: Rule; onChange(r: Rule): void }) {
  const m = rule.match ?? {};
  const setMatch = (patch: Partial<Rule['match']>) => onChange({ ...rule, match: { ...m, ...patch } });
  const addItems: MenuItemDef[] = (Object.keys(ACTION_LABEL) as RuleActionType[]).map((t) => ({ label: ACTION_LABEL[t], onSelect: () => onChange({ ...rule, actions: [...rule.actions, newAction(t)] }) }));
  return (
    <div className="col insp-rule-editor">
      <div className="row">
        <Field label="Name" className="grow">
          <input className="input" value={rule.name} onChange={(e) => onChange({ ...rule, name: e.target.value })} autoFocus />
        </Field>
        <Switch checked={rule.enabled !== false} onChange={(v) => onChange({ ...rule, enabled: v ? undefined : false })} label="Enabled" />
      </div>
      <section className="section">
        <div className="section-head">
          <h3>Match</h3>
          <span className="subtle xs">All set conditions must hold. Leave everything empty to match every request.</span>
        </div>
        <div className="grid-3">
          <Field label="Methods" hint="Comma-separated, empty = any">
            <input className="input sm mono" value={(m.methods ?? []).join(', ')} placeholder="GET, POST" onChange={(e) => setMatch({ methods: e.target.value.split(',').map((x) => x.trim().toUpperCase()).filter(Boolean) })} />
          </Field>
          <Field label="Host" hint="Glob, e.g. *.example.com">
            <input className="input sm mono" value={m.host ?? ''} placeholder="api.example.com" onChange={(e) => setMatch({ host: str(e.target.value) })} />
          </Field>
          <Field label="Path" hint="Glob without query, e.g. /api/*">
            <input className="input sm mono" value={m.path ?? ''} placeholder="/api/*" onChange={(e) => setMatch({ path: str(e.target.value) })} />
          </Field>
          <Field label="Path regex" hint="Searched in path + query">
            <input className="input sm mono" value={m.pathRegex ?? ''} placeholder="^/v[0-9]+/" onChange={(e) => setMatch({ pathRegex: str(e.target.value) })} />
          </Field>
          <Field label="Header name">
            <input className="input sm mono" value={m.header?.name ?? ''} placeholder="x-debug" onChange={(e) => setMatch({ header: e.target.value.trim() ? { name: e.target.value, value: m.header?.value } : undefined })} />
          </Field>
          <Field label="Header value" hint="Glob; empty = header present">
            <input className="input sm mono" value={m.header?.value ?? ''} disabled={!m.header} onChange={(e) => m.header && setMatch({ header: { name: m.header.name, value: str(e.target.value) } })} />
          </Field>
        </div>
      </section>
      <section className="section">
        <div className="section-head">
          <h3>Actions</h3>
          <span className="spacer" />
          <Menu
            trigger={(p) => (
              <button className="btn btn-sm" {...p}>
                <Plus /> Add action
              </button>
            )}
            items={addItems}
            align="right"
          />
        </div>
        {rule.actions.length === 0 && <p className="muted small">No actions: matching requests are only marked.</p>}
        {rule.actions.map((a, i) => (
          <div key={i} className="card insp-action">
            <div className="row-sm mb-2">
              <span className="badge accent">{ACTION_LABEL[a.type]}</span>
              <span className="spacer" />
              <button className="icon-btn sm danger" onClick={() => onChange({ ...rule, actions: rule.actions.filter((_, j) => j !== i) })} title="Remove action" aria-label="Remove action">
                <X />
              </button>
            </div>
            <ActionEditor a={a} onChange={(na) => onChange({ ...rule, actions: rule.actions.map((x, j) => (j === i ? na : x)) })} />
          </div>
        ))}
      </section>
    </div>
  );
}

/** Rules editor (list + form). Changes apply when saved. */
export function RulesPanel({ open, onClose, seed }: { open: boolean; onClose(): void; seed?: Rule | null }) {
  const stored = useInspector((s) => s.rules);
  const setStored = useInspector((s) => s.setRules);
  const [draft, setDraft] = useState<Rule[]>(stored);
  const [sel, setSel] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open) return;
    if (seed) {
      setDraft([...stored, seed]);
      setSel(stored.length);
    } else {
      setDraft(stored);
      setSel(0);
    }
    setError(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, seed]);

  const dirty = useMemo(() => JSON.stringify(draft) !== JSON.stringify(stored), [draft, stored]);
  const current = draft[sel];

  const save = async () => {
    setSaving(true);
    try {
      const saved = await proxyApi.saveRules(draft);
      setStored(saved);
      setDraft(saved);
      setError(null);
      toast.success('Rules saved', `${saved.filter((r) => r.enabled !== false).length} active`);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const close = async () => {
    if (dirty && !(await confirmDialog({ title: 'Discard rule changes?', message: 'Your edits to the rules have not been saved.', confirmText: 'Discard', danger: true }))) return;
    onClose();
  };

  const move = (d: -1 | 1) => {
    const j = sel + d;
    if (j < 0 || j >= draft.length) return;
    const next = draft.slice();
    [next[sel], next[j]] = [next[j], next[sel]];
    setDraft(next);
    setSel(j);
  };

  const exportRules = async () => {
    try {
      if (dirty) await save();
      const text = await proxyApi.exportRules();
      const path = await saveDialog({ defaultPath: 'devall-proxy-rules.json', filters: [{ name: 'JSON', extensions: ['json'] }] });
      if (!path) return;
      await api.writeTextFile(path, text);
      toast.success('Rules exported', path);
    } catch (e) {
      toast.error('Export failed', errorMessage(e));
    }
  };

  const importRules = async () => {
    try {
      const path = await openDialog({ multiple: false, filters: [{ name: 'JSON', extensions: ['json'] }] });
      if (typeof path !== 'string') return;
      const text = await api.readTextFile(path);
      const rules = await proxyApi.importRules(text, false);
      setStored(rules);
      setDraft(rules);
      toast.success('Rules imported', path);
    } catch (e) {
      toast.error('Import failed', errorMessage(e));
    }
  };

  const templates: MenuItemDef[] = (Object.keys(ACTION_LABEL) as RuleActionType[]).map((t) => ({
    label: ACTION_LABEL[t],
    onSelect: () => {
      setDraft([...draft, newRule({}, t)]);
      setSel(draft.length);
    },
  }));

  return (
    <Modal
      open={open}
      onClose={close}
      size="xl"
      title={
        <span className="row-sm">
          <Wand2 size={16} /> Rules
        </span>
      }
      bodyClassName="insp-rules-body"
      headerExtra={
        <div className="row-sm">
          <button className="btn btn-sm btn-ghost" onClick={importRules} title="Import rules from a JSON file">
            <Upload /> Import
          </button>
          <button className="btn btn-sm btn-ghost" onClick={exportRules} title="Export rules as JSON">
            <Download /> Export
          </button>
        </div>
      }
      footer={
        <>
          {error && <span className="text-danger small grow truncate">{error}</span>}
          <span className="spacer" />
          <button className="btn" onClick={close}>
            Close
          </button>
          <button className="btn btn-primary" onClick={save} disabled={!dirty || saving}>
            <Save /> Save rules
          </button>
        </>
      }
    >
      <div className="insp-rules">
        <div className="insp-rules-list">
          <div className="toolbar">
            <span className="strong small grow">Order matters</span>
            <Menu
              trigger={(p) => (
                <button className="icon-btn sm" title="New rule" aria-label="New rule" {...p}>
                  <Plus />
                </button>
              )}
              items={templates}
              align="right"
            />
          </div>
          {draft.length === 0 ? (
            <Empty icon={<Wand2 />} title="No rules yet">
              Pause, mock, rewrite, redirect, throttle or block traffic — free, locally.
            </Empty>
          ) : (
            <div className="list" role="listbox" aria-label="Rules">
              {draft.map((r, i) => (
                <div
                  key={r.id || `new-${i}`}
                  role="option"
                  aria-selected={i === sel}
                  tabIndex={0}
                  className={clsx('list-item insp-rule-item', i === sel && 'on', r.enabled === false && 'off')}
                  onClick={() => setSel(i)}
                  onKeyDown={(e) => {
                    if (e.key === 'ArrowDown') setSel(Math.min(draft.length - 1, i + 1));
                    if (e.key === 'ArrowUp') setSel(Math.max(0, i - 1));
                  }}
                >
                  <span onClick={(e) => e.stopPropagation()}>
                    <Switch checked={r.enabled !== false} onChange={(v) => setDraft(draft.map((x, j) => (j === i ? { ...x, enabled: v ? undefined : false } : x)))} />
                  </span>
                  <div className="grow col-sm" style={{ gap: 0 }}>
                    <span className="truncate small strong">{r.name || 'Unnamed rule'}</span>
                    <span className="truncate xs subtle mono">{summarize(r)}</span>
                  </div>
                  <span className="xs subtle">{r.actions.map((a) => ACTION_LABEL[a.type]).join(', ')}</span>
                </div>
              ))}
            </div>
          )}
          {current && (
            <div className="toolbar insp-rule-tools">
              <button className="icon-btn sm" onClick={() => move(-1)} disabled={sel === 0} title="Move up" aria-label="Move up">
                <ArrowUp />
              </button>
              <button className="icon-btn sm" onClick={() => move(1)} disabled={sel === draft.length - 1} title="Move down" aria-label="Move down">
                <ArrowDown />
              </button>
              <button
                className="icon-btn sm"
                onClick={() => {
                  setDraft([...draft.slice(0, sel + 1), { ...current, id: '', name: `${current.name} copy` }, ...draft.slice(sel + 1)]);
                  setSel(sel + 1);
                }}
                title="Duplicate"
                aria-label="Duplicate"
              >
                <Copy />
              </button>
              <span className="spacer" />
              <button
                className="icon-btn sm danger"
                onClick={() => {
                  setDraft(draft.filter((_, j) => j !== sel));
                  setSel(Math.max(0, sel - 1));
                }}
                title="Delete rule"
                aria-label="Delete rule"
              >
                <Trash2 />
              </button>
            </div>
          )}
        </div>
        <div className="insp-rules-form">
          {current ? (
            <RuleEditor rule={current} onChange={(r) => setDraft(draft.map((x, j) => (j === sel ? r : x)))} />
          ) : (
            <Callout>Rules apply to traffic through the proxy in order: header edits add up; the first Map Remote, the first mock / Map Local / block and the first throttle win; any matching breakpoint pauses.</Callout>
          )}
        </div>
      </div>
    </Modal>
  );
}
