import { useMemo } from 'react';
import clsx from 'clsx';
import { ArrowDown, ArrowUp, Plus, Trash2 } from 'lucide-react';
import { Field, MethodBadge, Segmented, Switch } from '@/components/ui/misc';
import type { Check, Executor, ExecutorType, Issue, LoadTestFile, Stage, ThinkTime } from '@/lib/api/load';
import type { EnvironmentEntry, TreeNode } from '@/lib/types';
import { ProfilePreview } from './charts';
import { EXECUTOR_INFO, profilePoints } from './format';

export interface RequestOption {
  path: string;
  name: string;
  method: string;
  folder: string;
}

export function flattenRequests(node: TreeNode | null, out: RequestOption[] = [], folder = ''): RequestOption[] {
  if (!node) return out;
  if (node.kind === 'request') out.push({ path: node.path, name: node.name, method: node.method, folder });
  else for (const c of node.children) flattenRequests(c, out, node.path ? (folder ? `${folder} / ${node.name}` : node.name) : '');
  return out;
}

interface Props {
  test: LoadTestFile;
  onChange(t: LoadTestFile): void;
  issues: Issue[];
  requests: RequestOption[];
  environments: EnvironmentEntry[];
  activeEnvName?: string;
}

const THRESHOLD_PRESETS = ['p95 < 300ms', 'p99 < 1s', 'errorRate < 1%', 'checks > 99%', 'rps > 50', 'dropped == 0'];

function issuesAt(issues: Issue[], field: string) {
  return issues.filter((i) => i.field === field);
}

function FieldIssues({ issues, field }: { issues: Issue[]; field: string }) {
  const list = issuesAt(issues, field);
  if (!list.length) return null;
  return (
    <>
      {list.map((i, k) => (
        <div key={k} className="field-issue" role="alert">
          {i.message}
        </div>
      ))}
    </>
  );
}

function NumInput({ value, onChange, min = 0, step = 1, placeholder, invalid, label }: { value: number | undefined; onChange(v: number | undefined): void; min?: number; step?: number; placeholder?: string; invalid?: boolean; label: string }) {
  return (
    <input
      className={clsx('input sm', invalid && 'invalid')}
      type="number"
      inputMode="decimal"
      min={min}
      step={step}
      value={value ?? ''}
      placeholder={placeholder}
      aria-label={label}
      onChange={(e) => onChange(e.target.value === '' ? undefined : Number(e.target.value))}
    />
  );
}

function DurInput({ value, onChange, invalid, label, placeholder = '30s' }: { value: string | undefined; onChange(v: string): void; invalid?: boolean; label: string; placeholder?: string }) {
  return <input className={clsx('input sm mono', invalid && 'invalid')} value={value ?? ''} placeholder={placeholder} aria-label={label} spellCheck={false} onChange={(e) => onChange(e.target.value)} />;
}

function convertExecutor(from: Executor, to: ExecutorType): Executor {
  const duration = 'duration' in from ? from.duration : '1m';
  const vus = from.type === 'constantVus' ? from.vus : from.type === 'rampingVus' ? Math.max(1, ...from.stages.map((s) => s.target)) : 10;
  const pool = 'preAllocatedVus' in from ? { preAllocatedVus: from.preAllocatedVus, maxVus: from.maxVus } : { preAllocatedVus: 10, maxVus: 50 };
  switch (to) {
    case 'constantVus':
      return { type: 'constantVus', vus, duration };
    case 'rampingVus':
      return { type: 'rampingVus', startVus: 0, stages: [{ duration: '30s', target: vus }, { duration: '1m', target: vus }, { duration: '30s', target: 0 }] };
    case 'constantArrivalRate':
      return { type: 'constantArrivalRate', rate: from.type === 'rampingArrivalRate' ? Math.max(1, ...from.stages.map((s) => s.target)) : 10, duration, ...pool };
    case 'rampingArrivalRate': {
      const r = from.type === 'constantArrivalRate' ? from.rate : 20;
      return { type: 'rampingArrivalRate', startRate: 0, stages: [{ duration: '30s', target: r }, { duration: '1m', target: r }, { duration: '30s', target: 0 }], ...pool };
    }
  }
}

export function TestEditor({ test, onChange, issues, requests, environments, activeEnvName }: Props) {
  const set = (patch: Partial<LoadTestFile>) => onChange({ ...test, ...patch });
  const byPath = useMemo(() => new Map(requests.map((r) => [r.path, r])), [requests]);
  const folders = useMemo(() => {
    const m = new Map<string, RequestOption[]>();
    for (const r of requests) m.set(r.folder, [...(m.get(r.folder) ?? []), r]);
    return [...m.entries()];
  }, [requests]);
  const isFlow = (test.scenario.flow?.length ?? 0) > 0 || (!test.scenario.requests?.length && test.scenario.flow !== undefined);
  const weighted = test.scenario.requests ?? [];
  const flow = test.scenario.flow ?? [];
  const totalWeight = weighted.reduce((a, r) => a + (r.weight ?? 1), 0);
  const ex = test.executor;
  const setEx = (patch: Partial<Executor>) => set({ executor: { ...ex, ...patch } as Executor });
  const profile = profilePoints(ex);
  const rateUnit = 'timeUnit' in ex && ex.timeUnit && ex.timeUnit !== '1s' ? ex.timeUnit : '1s';

  const reqLabel = (path: string) => {
    const r = byPath.get(path);
    return (
      <span className={clsx('load-req', !r && 'missing')} title={path}>
        <MethodBadge method={r?.method} short />
        <span className="truncate">{r?.name ?? path}</span>
        {r && <span className="subtle xs truncate">{path}</span>}
        {!r && <span className="xs">(not found)</span>}
      </span>
    );
  };

  const picker = (onPick: (path: string) => void, exclude: string[] = []) => (
    <select
      className="select sm"
      style={{ maxWidth: 360 }}
      value=""
      aria-label="Add a request"
      onChange={(e) => {
        if (e.target.value) onPick(e.target.value);
      }}
    >
      <option value="">{requests.length ? 'Add a request…' : 'No requests in this project yet'}</option>
      {folders.map(([folder, list]) => (
        <optgroup key={folder || '/'} label={folder || 'Project root'}>
          {list
            .filter((r) => !exclude.includes(r.path))
            .map((r) => (
              <option key={r.path} value={r.path}>
                {r.method.toUpperCase()} {r.name}
              </option>
            ))}
        </optgroup>
      ))}
    </select>
  );

  const stagesEditor = (stages: Stage[], unitLabel: string, onStages: (s: Stage[]) => void) => (
    <div className="load-rows">
      {stages.map((s, i) => (
        <div key={i}>
          <div className="load-row lr-stage">
            <span className="idx">{i + 1}</span>
            <DurInput label={`Stage ${i + 1} duration`} value={s.duration} invalid={issuesAt(issues, `executor.stages[${i}].duration`).length > 0} onChange={(v) => onStages(stages.map((x, j) => (j === i ? { ...x, duration: v } : x)))} />
            <NumInput label={`Stage ${i + 1} target`} value={s.target} invalid={issuesAt(issues, `executor.stages[${i}].target`).length > 0} onChange={(v) => onStages(stages.map((x, j) => (j === i ? { ...x, target: v ?? 0 } : x)))} />
            <span className="subtle xs">
              ramp to {s.target} {unitLabel} over {s.duration || '…'}
            </span>
            <button className="icon-btn sm danger" onClick={() => onStages(stages.filter((_, j) => j !== i))} aria-label={`Remove stage ${i + 1}`} title="Remove stage">
              <Trash2 />
            </button>
          </div>
          <FieldIssues issues={issues} field={`executor.stages[${i}].duration`} />
          <FieldIssues issues={issues} field={`executor.stages[${i}].target`} />
        </div>
      ))}
      <div className="row">
        <button className="btn btn-sm btn-ghost" onClick={() => onStages([...stages, stages.length ? { ...stages[stages.length - 1] } : { duration: '30s', target: 10 }])}>
          <Plus /> Add stage
        </button>
      </div>
      <FieldIssues issues={issues} field="executor.stages" />
    </div>
  );

  const thinkMode: 'none' | 'fixed' | 'range' = test.thinkTime == null ? 'none' : typeof test.thinkTime === 'string' ? 'fixed' : 'range';
  const setThink = (t: ThinkTime | undefined) => set({ thinkTime: t });
  const thresholds = test.thresholds ?? [];
  const checks = test.checks ?? [];
  const http = test.http ?? {};
  const setHttp = (patch: Partial<typeof http>) => {
    const next = { ...http, ...patch };
    for (const k of Object.keys(next) as (keyof typeof next)[]) if (next[k] === undefined || next[k] === false) delete next[k];
    set({ http: Object.keys(next).length ? next : undefined });
  };

  return (
    <div className="load-editor">
      {/* ---------- general ---------- */}
      <section className="load-section">
        <div className="load-fields">
          <Field label="Description" className="wide">
            <input className="input sm" value={test.description ?? ''} placeholder="What this test proves" onChange={(e) => set({ description: e.target.value || undefined })} />
          </Field>
          <Field label="Environment" hint={test.environment ? 'Pinned by the test; the CLI can override with --env.' : 'Uses the active environment.'}>
            <select className="select sm" value={test.environment ?? ''} onChange={(e) => set({ environment: e.target.value || undefined })}>
              <option value="">Active environment{activeEnvName ? ` (${activeEnvName})` : ''}</option>
              {environments.map((e) => (
                <option key={e.id} value={e.id}>
                  {e.file.name}
                </option>
              ))}
            </select>
          </Field>
          <Field
            label="Target"
            hint={test.target === 'mock' ? 'Every run goes to the project mock server (mocks/), started if needed: a baseline of the client side.' : 'The saved requests’ own hosts. “Run against mock” makes a one-off baseline.'}
          >
            <select className="select sm" value={test.target ?? 'requests'} onChange={(e) => set({ target: e.target.value === 'mock' ? 'mock' : undefined })}>
              <option value="requests">Request URLs (environment)</option>
              <option value="mock">Project mock server</option>
            </select>
          </Field>
        </div>
      </section>

      {/* ---------- scenario ---------- */}
      <section className="load-section">
        <div className="section-head">
          <h3>Scenario</h3>
          <span className="spacer" />
          <Segmented
            value={isFlow ? 'flow' : 'mix'}
            onChange={(v) => {
              if (v === 'flow' && !isFlow) set({ scenario: { flow: weighted.map((r) => r.path) } });
              if (v === 'mix' && isFlow) set({ scenario: { requests: flow.filter((p, i) => flow.indexOf(p) === i).map((path) => ({ path })) } });
            }}
            options={[
              { value: 'mix', label: 'Weighted mix', title: 'Each iteration sends one request, picked at random by weight' },
              { value: 'flow', label: 'Flow', title: 'Each iteration runs every step in order (cookies carry over between steps)' },
            ]}
          />
        </div>
        <p className="load-help">
          {isFlow
            ? 'Every iteration runs these steps in order, with a fresh cookie jar, so login → action flows work. Templates like {{$uuid}} resolve anew for every request.'
            : 'Every iteration sends one request, picked at random by weight. Templates like {{$uuid}} resolve anew for every request.'}
        </p>
        <div className="load-rows">
          {!isFlow &&
            weighted.map((r, i) => (
              <div key={`${r.path}-${i}`}>
                <div className="load-row">
                  <span className="idx">{i + 1}</span>
                  {reqLabel(r.path)}
                  <NumInput
                    label={`Weight of ${r.path}`}
                    min={1}
                    value={r.weight ?? 1}
                    invalid={issuesAt(issues, `scenario.requests[${i}].weight`).length > 0}
                    onChange={(v) => set({ scenario: { requests: weighted.map((x, j) => (j === i ? { path: x.path, ...(v != null && v !== 1 ? { weight: Math.max(0, Math.round(v)) } : {}) } : x)) } })}
                  />
                  <span className="share">{totalWeight ? `${Math.round(((r.weight ?? 1) / totalWeight) * 100)}%` : ''}</span>
                  <button className="icon-btn sm danger" onClick={() => set({ scenario: { requests: weighted.filter((_, j) => j !== i) } })} aria-label={`Remove ${r.path}`} title="Remove">
                    <Trash2 />
                  </button>
                </div>
                <FieldIssues issues={issues} field={`scenario.requests[${i}].path`} />
                <FieldIssues issues={issues} field={`scenario.requests[${i}].weight`} />
              </div>
            ))}
          {isFlow &&
            flow.map((p, i) => (
              <div key={`${p}-${i}`}>
                <div className="load-row lr-flow">
                  <span className="idx">{i + 1}</span>
                  {reqLabel(p)}
                  <span className="row-sm">
                    <button
                      className="icon-btn sm"
                      disabled={i === 0}
                      aria-label={`Move step ${i + 1} up`}
                      onClick={() => {
                        const f = [...flow];
                        [f[i - 1], f[i]] = [f[i], f[i - 1]];
                        set({ scenario: { flow: f } });
                      }}
                    >
                      <ArrowUp />
                    </button>
                    <button
                      className="icon-btn sm"
                      disabled={i === flow.length - 1}
                      aria-label={`Move step ${i + 1} down`}
                      onClick={() => {
                        const f = [...flow];
                        [f[i + 1], f[i]] = [f[i], f[i + 1]];
                        set({ scenario: { flow: f } });
                      }}
                    >
                      <ArrowDown />
                    </button>
                  </span>
                  <button className="icon-btn sm danger" onClick={() => set({ scenario: { flow: flow.filter((_, j) => j !== i) } })} aria-label={`Remove step ${i + 1}`} title="Remove">
                    <Trash2 />
                  </button>
                </div>
                <FieldIssues issues={issues} field={`scenario.flow[${i}]`} />
              </div>
            ))}
          <div className="row">
            {picker(
              (path) => (isFlow ? set({ scenario: { flow: [...flow, path] } }) : set({ scenario: { requests: [...weighted, { path }] } })),
              isFlow ? [] : weighted.map((r) => r.path),
            )}
          </div>
          <FieldIssues issues={issues} field="scenario" />
        </div>
      </section>

      {/* ---------- executor ---------- */}
      <section className="load-section">
        <div className="section-head">
          <h3>Load profile</h3>
        </div>
        <div className="executor-pick" role="radiogroup" aria-label="Executor">
          {(Object.keys(EXECUTOR_INFO) as ExecutorType[]).map((t) => (
            <button key={t} role="radio" aria-checked={ex.type === t} className={clsx(ex.type === t && 'on')} onClick={() => ex.type !== t && set({ executor: convertExecutor(ex, t) })}>
              <span className="strong">{EXECUTOR_INFO[t].label}</span>
              <span className="xs">{EXECUTOR_INFO[t].open ? 'Open model · requests/s' : 'Closed model · users'}</span>
            </button>
          ))}
        </div>
        <p className="load-help">{EXECUTOR_INFO[ex.type].help}</p>
        <div className="load-fields">
          {ex.type === 'constantVus' && (
            <>
              <Field label="Virtual users" error={issuesAt(issues, 'executor.vus')[0]?.message}>
                <NumInput label="Virtual users" min={1} value={ex.vus} onChange={(v) => setEx({ vus: v ?? 0 })} />
              </Field>
              <Field label="Duration" error={issuesAt(issues, 'executor.duration')[0]?.message}>
                <DurInput label="Duration" value={ex.duration} onChange={(v) => setEx({ duration: v })} />
              </Field>
            </>
          )}
          {ex.type === 'rampingVus' && (
            <Field label="Start VUs" error={issuesAt(issues, 'executor.startVus')[0]?.message}>
              <NumInput label="Start VUs" value={ex.startVus ?? 0} onChange={(v) => setEx({ startVus: v || undefined })} />
            </Field>
          )}
          {(ex.type === 'constantArrivalRate' || ex.type === 'rampingArrivalRate') && (
            <>
              {ex.type === 'constantArrivalRate' ? (
                <Field label="Rate (iterations)" error={issuesAt(issues, 'executor.rate')[0]?.message}>
                  <NumInput label="Rate" min={0} step={0.5} value={ex.rate} onChange={(v) => setEx({ rate: v ?? 0 })} />
                </Field>
              ) : (
                <Field label="Start rate" error={issuesAt(issues, 'executor.startRate')[0]?.message}>
                  <NumInput label="Start rate" min={0} step={0.5} value={ex.startRate ?? 0} onChange={(v) => setEx({ startRate: v || undefined })} />
                </Field>
              )}
              <Field label="Per" error={issuesAt(issues, 'executor.timeUnit')[0]?.message}>
                <select className="select sm" value={rateUnit} onChange={(e) => setEx({ timeUnit: e.target.value === '1s' ? undefined : e.target.value })}>
                  <option value="1s">second</option>
                  <option value="1m">minute</option>
                  <option value="1h">hour</option>
                  {!['1s', '1m', '1h'].includes(rateUnit) && <option value={rateUnit}>{rateUnit}</option>}
                </select>
              </Field>
              {ex.type === 'constantArrivalRate' && (
                <Field label="Duration" error={issuesAt(issues, 'executor.duration')[0]?.message}>
                  <DurInput label="Duration" value={ex.duration} onChange={(v) => setEx({ duration: v })} />
                </Field>
              )}
              <Field label="Pre-allocated VUs" hint="Ready before the test starts" error={issuesAt(issues, 'executor.preAllocatedVus')[0]?.message}>
                <NumInput label="Pre-allocated VUs" min={1} value={ex.preAllocatedVus} onChange={(v) => setEx({ preAllocatedVus: v ?? 0 })} />
              </Field>
              <Field label="Max VUs" hint="Beyond this, late iterations are dropped and reported" error={issuesAt(issues, 'executor.maxVus')[0]?.message}>
                <NumInput label="Max VUs" min={1} value={ex.maxVus} placeholder={String(ex.preAllocatedVus)} onChange={(v) => setEx({ maxVus: v })} />
              </Field>
            </>
          )}
        </div>
        {ex.type === 'rampingVus' && stagesEditor(ex.stages, 'VUs', (stages) => setEx({ stages }))}
        {ex.type === 'rampingArrivalRate' && stagesEditor(ex.stages, `iterations/${rateUnit === '1s' ? 's' : rateUnit}`, (stages) => setEx({ stages }))}
        <FieldIssues issues={issues} field="executor" />
        <ProfilePreview points={profile.points} unit={profile.unit} />
      </section>

      {/* ---------- think time ---------- */}
      <section className="load-section">
        <div className="section-head">
          <h3>Think time</h3>
          <span className="spacer" />
          <Segmented
            value={thinkMode}
            onChange={(v) => setThink(v === 'none' ? undefined : v === 'fixed' ? '1s' : { min: '500ms', max: '2s' })}
            options={[
              { value: 'none', label: 'None' },
              { value: 'fixed', label: 'Fixed' },
              { value: 'range', label: 'Random range' },
            ]}
          />
        </div>
        <p className="load-help">A pause after every request: between flow steps and, for VU executors, between iterations. Arrival-rate tests start iterations on schedule regardless.</p>
        {thinkMode !== 'none' && (
          <div className="load-fields">
            {typeof test.thinkTime === 'string' && (
              <Field label="Pause" error={issuesAt(issues, 'thinkTime')[0]?.message}>
                <DurInput label="Think time" value={test.thinkTime} placeholder="1s" onChange={(v) => setThink(v)} />
              </Field>
            )}
            {test.thinkTime && typeof test.thinkTime === 'object' && (
              <>
                <Field label="Min" error={issuesAt(issues, 'thinkTime.min')[0]?.message}>
                  <DurInput label="Minimum think time" value={test.thinkTime.min} placeholder="500ms" onChange={(v) => setThink({ ...(test.thinkTime as { min: string; max: string }), min: v })} />
                </Field>
                <Field label="Max" error={issuesAt(issues, 'thinkTime.max')[0]?.message ?? issuesAt(issues, 'thinkTime')[0]?.message}>
                  <DurInput label="Maximum think time" value={test.thinkTime.max} placeholder="2s" onChange={(v) => setThink({ ...(test.thinkTime as { min: string; max: string }), max: v })} />
                </Field>
              </>
            )}
          </div>
        )}
      </section>

      {/* ---------- thresholds ---------- */}
      <section className="load-section">
        <div className="section-head">
          <h3>Thresholds</h3>
          <span className="subtle xs">Any failure fails the run (CLI exit code 99).</span>
        </div>
        <p className="load-help">
          <code className="code-inline">metric[{'{request path}'}] op value</code> — metrics: p50…p99.9, p(N), med, min, max, avg, rps, errorRate, checks, requests, errors, iterations, dropped.
        </p>
        <div className="load-rows">
          {thresholds.map((t, i) => (
            <div key={i}>
              <div className="load-row lr-threshold">
                <input
                  className={clsx('input sm mono', issuesAt(issues, `thresholds[${i}]`).length > 0 && 'invalid')}
                  value={t}
                  spellCheck={false}
                  aria-label={`Threshold ${i + 1}`}
                  placeholder="p95 < 300ms"
                  onChange={(e) => set({ thresholds: thresholds.map((x, j) => (j === i ? e.target.value : x)) })}
                />
                <button className="icon-btn sm danger" onClick={() => set({ thresholds: thresholds.filter((_, j) => j !== i) })} aria-label={`Remove threshold ${i + 1}`} title="Remove">
                  <Trash2 />
                </button>
              </div>
              <FieldIssues issues={issues} field={`thresholds[${i}]`} />
            </div>
          ))}
          <div className="load-chips">
            {THRESHOLD_PRESETS.filter((p) => !thresholds.includes(p)).map((p) => (
              <button key={p} className="tag mono" onClick={() => set({ thresholds: [...thresholds, p] })} title="Add threshold">
                <Plus /> {p}
              </button>
            ))}
            <button className="tag" onClick={() => set({ thresholds: [...thresholds, ''] })}>
              <Plus /> Custom
            </button>
          </div>
        </div>
      </section>

      {/* ---------- checks ---------- */}
      <section className="load-section">
        <div className="section-head">
          <h3>Checks</h3>
          <span className="subtle xs">{checks.length ? 'Evaluated on every response; feed the “checks” metric.' : 'Default: the response status is below 400.'}</span>
        </div>
        <div className="load-rows">
          {checks.map((c, i) => (
            <div key={i}>
              <div className="load-row lr-check">
                <select
                  className="select sm"
                  value={c.type}
                  aria-label={`Check ${i + 1} type`}
                  onChange={(e) => {
                    const type = e.target.value as Check['type'];
                    const next: Check = type === 'status' ? { type, expect: ['2xx'] } : type === 'maxDuration' ? { type, duration: '500ms' } : { type, text: '' };
                    set({ checks: checks.map((x, j) => (j === i ? next : x)) });
                  }}
                >
                  <option value="status">Status is</option>
                  <option value="maxDuration">Responds within</option>
                  <option value="bodyContains">Body contains</option>
                </select>
                {c.type === 'status' && (
                  <input
                    className={clsx('input sm mono', issues.some((x) => x.field.startsWith(`checks[${i}]`)) && 'invalid')}
                    value={c.expect.join(', ')}
                    placeholder="2xx, 304"
                    aria-label={`Check ${i + 1} statuses`}
                    onChange={(e) => set({ checks: checks.map((x, j) => (j === i ? { type: 'status', expect: e.target.value.split(/[,\s]+/).filter(Boolean) } : x)) })}
                  />
                )}
                {c.type === 'maxDuration' && <DurInput label={`Check ${i + 1} duration`} value={c.duration} placeholder="500ms" onChange={(v) => set({ checks: checks.map((x, j) => (j === i ? { type: 'maxDuration', duration: v } : x)) })} />}
                {c.type === 'bodyContains' && (
                  <input className="input sm mono" value={c.text} placeholder='"ok":true' aria-label={`Check ${i + 1} text`} onChange={(e) => set({ checks: checks.map((x, j) => (j === i ? { type: 'bodyContains', text: e.target.value } : x)) })} />
                )}
                <button className="icon-btn sm danger" onClick={() => set({ checks: checks.filter((_, j) => j !== i).length ? checks.filter((_, j) => j !== i) : undefined })} aria-label={`Remove check ${i + 1}`} title="Remove">
                  <Trash2 />
                </button>
              </div>
              {issues
                .filter((x) => x.field.startsWith(`checks[${i}]`))
                .map((x, k) => (
                  <div key={k} className="field-issue">
                    {x.message}
                  </div>
                ))}
            </div>
          ))}
          <div className="row">
            <button className="btn btn-sm btn-ghost" onClick={() => set({ checks: [...checks, { type: 'status', expect: ['2xx'] }] })}>
              <Plus /> Add check
            </button>
          </div>
        </div>
      </section>

      {/* ---------- http ---------- */}
      <section className="load-section">
        <div className="section-head">
          <h3>HTTP</h3>
        </div>
        <div className="load-fields">
          <Field label="Timeout (ms)" hint="Default: each request's own setting" error={issuesAt(issues, 'http.timeoutMs')[0]?.message}>
            <NumInput label="Timeout in milliseconds" min={1} value={http.timeoutMs} placeholder="per request" onChange={(v) => setHttp({ timeoutMs: v })} />
          </Field>
          <Field label="Max connections" hint="Caps requests in flight; waiting counts as latency" error={issuesAt(issues, 'http.maxConnections')[0]?.message}>
            <NumInput label="Max connections" min={1} value={http.maxConnections} placeholder="unlimited" onChange={(v) => setHttp({ maxConnections: v })} />
          </Field>
        </div>
        <div className="row-lg wrap">
          <Switch checked={!!http.http2} onChange={(v) => setHttp({ http2: v })} label="HTTP/2 (prior knowledge, h2c for http://)" />
          <Switch checked={!!http.insecure} onChange={(v) => setHttp({ insecure: v })} label="Accept invalid TLS certificates" />
        </div>
      </section>
    </div>
  );
}
