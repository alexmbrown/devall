import { useMemo, useState } from 'react';
import { useQuery, useQueryClient } from '@/features/mocks/lib/query';
import clsx from 'clsx';
import { GitCompare, ArrowRight, ArrowLeft, RefreshCw } from 'lucide-react';
import type { StubMapping } from '@/features/mocks/lib/types';
import { useTargets, useMockTarget, serverKey, BUILTIN_ID, type Target } from '@/features/mocks/store/target';
import { clientFor, qk } from '@/features/mocks/lib/queries';
import { useNav } from '@/features/mocks/store/nav';
import { cleanForSave, displayName, methodLabel, prettyJson, stubId, urlOf } from '@/features/mocks/lib/stub-utils';
import { MethodBadge, Spinner, Checkbox, Empty, Segmented } from '@/features/mocks/components/ui';
import { CodeDiff } from '@/components/editor/CodeEditor';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';

type Status = 'same' | 'changed' | 'onlyA' | 'onlyB';
interface Row {
  key: string;
  status: Status;
  a?: StubMapping;
  b?: StubMapping;
}

/** Stubs are paired by id, falling back to method + URL + name for stubs recreated with new ids. */
function pairKey(s: StubMapping) {
  return `${methodLabel(s.request.method)} ${urlOf(s.request)} ${s.name ?? ''} ${s.scenarioName ?? ''}:${s.requiredScenarioState ?? ''}`;
}

function normalized(s: StubMapping) {
  const c = cleanForSave(s);
  delete c.id;
  delete c.uuid;
  return prettyJson(c);
}

export default function ComparePage() {
  const all = useTargets();
  const activeId = useMockTarget((s) => s.activeId);
  // Default: this project vs the active remote (or the first remote) — e.g. pull a team's shared stubs into the repo.
  const [aId, setA] = useState(BUILTIN_ID);
  const [bId, setB] = useState(() => (activeId !== BUILTIN_ID ? activeId : (all[1]?.id ?? BUILTIN_ID)));
  const A = all.find((x) => x.id === aId);
  const B = all.find((x) => x.id === bId);
  const qa = useServerMappings(A);
  const qb = useServerMappings(B);
  const [filter, setFilter] = useState<'diff' | 'all'>('diff');
  const [sel, setSel] = useState<string | null>(null);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const qc = useQueryClient();

  const rows = useMemo<Row[]>(() => {
    if (!qa.data || !qb.data) return [];
    const byIdB = new Map(qb.data.map((s) => [stubId(s), s]));
    const byKeyB = new Map(qb.data.map((s) => [pairKey(s), s]));
    const used = new Set<string>();
    const out: Row[] = [];
    for (const a of qa.data) {
      const b = byIdB.get(stubId(a)) ?? (byKeyB.get(pairKey(a)) && !used.has(stubId(byKeyB.get(pairKey(a))!)) ? byKeyB.get(pairKey(a)) : undefined);
      if (b) used.add(stubId(b));
      out.push({ key: stubId(a), a, b, status: !b ? 'onlyA' : normalized(a) === normalized(b) ? 'same' : 'changed' });
    }
    for (const b of qb.data) if (!used.has(stubId(b))) out.push({ key: `b:${stubId(b)}`, b, status: 'onlyB' });
    const order: Record<Status, number> = { changed: 0, onlyA: 1, onlyB: 2, same: 3 };
    return out.sort((x, y) => order[x.status] - order[y.status]);
  }, [qa.data, qb.data]);

  const visible = rows.filter((r) => filter === 'all' || r.status !== 'same');
  const counts = rows.reduce((m, r) => ({ ...m, [r.status]: (m[r.status] ?? 0) + 1 }), {} as Record<Status, number>);
  const current = rows.find((r) => r.key === sel);

  const copy = async (dir: 'AtoB' | 'BtoA') => {
    const target = dir === 'AtoB' ? B : A;
    if (!target) return;
    if (target.readOnly) return toast.error(`${target.name} is read-only`);
    const stubs = rows.filter((r) => checked.has(r.key)).map((r) => (dir === 'AtoB' ? r.a : r.b)).filter(Boolean) as StubMapping[];
    if (!stubs.length) return toast.info(`Select stubs that exist on ${dir === 'AtoB' ? A?.name : B?.name}`);
    const c = clientFor(target);
    if (!c) return toast.error(`${target.name} is not running`);
    const ok = await confirmDialog({
      title: `Copy ${stubs.length} stub(s) to ${target.name}?`,
      message: target.id === BUILTIN_ID ? 'They are saved as files in mocks/mappings/. Stubs with the same id are overwritten.' : 'Stubs with the same id are overwritten.',
      confirmText: 'Copy',
      typeToConfirm: target.protected ? target.name : undefined,
    });
    if (!ok) return;
    try {
      const toBuiltin = target.id === BUILTIN_ID;
      await c.importMappings(stubs.map((s) => cleanForSave(toBuiltin && s.persistent === undefined ? { ...s, persistent: true } : s)), 'OVERWRITE');
      if (toBuiltin) {
        useMockTarget.getState().markLocalWrite();
        void useMockTarget.getState().refreshStatus();
      }
      toast.success(`Copied ${stubs.length} stub(s) to ${target.name}`);
      qc.invalidateQueries({ queryKey: ['compare'] });
      qc.invalidateQueries({ queryKey: qk.all(serverKey(target)) });
      setChecked(new Set());
    } catch (e) {
      toast.error('Copy failed', e);
    }
  };

  if (all.length < 2)
    return (
      <Empty
        icon={<GitCompare />}
        title="Add a remote WireMock to compare"
        action={
          <button className="btn btn-primary" onClick={() => useNav.getState().go('servers', 'new')}>
            Add a remote server
          </button>
        }
      >
        Compare this project's stubs with a team or staging WireMock, spot drift, and copy stubs either way — e.g. pull shared stubs into the repo.
      </Empty>
    );

  return (
    <div className="page">
      <div className="page-header">
        <GitCompare size={16} className="subtle" />
        <h1>Compare servers</h1>
        <ServerPicker all={all} value={aId} onChange={setA} />
        <span className="subtle">vs</span>
        <ServerPicker all={all} value={bId} onChange={setB} />
        <button className="icon-btn" onClick={() => (qa.refetch(), qb.refetch())} title="Refresh">
          {qa.isFetching || qb.isFetching ? <Spinner /> : <RefreshCw />}
        </button>
        <div className="spacer" />
        <button className="btn btn-sm" onClick={() => copy('AtoB')} disabled={!checked.size}>
          Copy to {B?.name} <ArrowRight />
        </button>
        <button className="btn btn-sm" onClick={() => copy('BtoA')} disabled={!checked.size}>
          <ArrowLeft /> Copy to {A?.name}
        </button>
      </div>
      <div className="toolbar">
        <Segmented value={filter} onChange={setFilter} options={[{ value: 'diff', label: 'Differences' }, { value: 'all', label: 'All' }]} />
        <span className="badge warning">{counts.changed ?? 0} changed</span>
        <span className="badge info">{counts.onlyA ?? 0} only in {A?.name}</span>
        <span className="badge accent">{counts.onlyB ?? 0} only in {B?.name}</span>
        <span className="badge outline">{counts.same ?? 0} identical</span>
      </div>
      {(qa.isError || qb.isError) && <div className="p-4 text-danger small">{String(((qa.error ?? qb.error) as Error)?.message)}</div>}
      <div className="split">
        <div className="pane side" style={{ width: 460 }}>
          <div className="list">
            {(qa.isPending || qb.isPending) && <Spinner />}
            {visible.map((r) => {
              const s = r.a ?? r.b!;
              return (
                <div key={r.key} className={clsx('list-item', sel === r.key && 'on')} onClick={() => setSel(r.key)}>
                  <Checkbox
                    checked={checked.has(r.key)}
                    onChange={(v) => {
                      const n = new Set(checked);
                      if (v) n.add(r.key);
                      else n.delete(r.key);
                      setChecked(n);
                    }}
                  />
                  <MethodBadge method={s.request.method} />
                  <span className="truncate grow">{displayName(s)}</span>
                  <span className={clsx('badge', r.status === 'changed' ? 'warning' : r.status === 'onlyA' ? 'info' : r.status === 'onlyB' ? 'accent' : 'outline')}>
                    {r.status === 'changed' ? 'changed' : r.status === 'onlyA' ? `only ${A?.name}` : r.status === 'onlyB' ? `only ${B?.name}` : 'same'}
                  </span>
                </div>
              );
            })}
            {qa.data && qb.data && visible.length === 0 && <div className="p-5 subtle small">No differences. 🎉</div>}
          </div>
        </div>
        <div className="pane fill p-4">
          {current ? (
            <div className="col grow min-h-0" style={{ height: '100%' }}>
              <div className="row subtle xs">
                <span className="grow">Left: {A?.name}</span>
                <span>Right: {B?.name}</span>
              </div>
              <div className="editor-shell fill">
                <CodeDiff original={current.a ? normalized(current.a) : ''} modified={current.b ? normalized(current.b) : ''} />
              </div>
            </div>
          ) : (
            <Empty title="Select a stub to see the diff">Ids are ignored when comparing, so stubs recreated on another server still pair up by method, URL and name.</Empty>
          )}
        </div>
      </div>
    </div>
  );
}

function useServerMappings(s?: Target) {
  return useQuery({
    queryKey: ['compare', s?.id, s?.url],
    queryFn: async () => {
      const c = s ? clientFor(s) : undefined;
      if (!c) throw new Error(`${s?.name ?? 'Server'} is not running`);
      return (await c.mappings()).mappings;
    },
    enabled: !!s,
    retry: false,
  });
}

function ServerPicker({ all, value, onChange }: { all: Target[]; value?: string; onChange(v: string): void }) {
  return (
    <select className="select sm" style={{ width: 220 }} value={value} onChange={(e) => onChange(e.target.value)}>
      {all.map((t) => (
        <option key={t.id} value={t.id}>
          {t.kind === 'builtin' ? 'This project (built-in)' : t.name}
        </option>
      ))}
    </select>
  );
}
