import { useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { ArrowDown, ArrowUp, ChefHat, ClipboardCopy, FolderOpen, Import, Play, Plus, Save, Sparkles, Trash2, Wand2, X } from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { Menu } from '@/components/ui/Menu';
import { Callout, CopyButton, Empty, Spinner, Switch, copyText } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { defaultArgs, utilsApi, type MagicResult, type OpInfo, type RecipeStep, type UtilArgs } from '@/lib/api/utils';
import { ArgField, NetworkBadge, PaneHead, ToolHeader, loadLocal, outputLanguage, saveLocal, sizeLabel, useSeed, useUtilities, type ToolProps } from './shared';

interface Step extends RecipeStep {
  key: number;
}

interface StepResult {
  output?: string;
  error?: string;
  skipped?: boolean;
}

interface SavedRecipe {
  name: string;
  steps: RecipeStep[];
}

const CURRENT_KEY = 'devall.utils.recipe.current';
const SAVED_KEY = 'devall.utils.recipes';
const PREVIEW = 600;
let nextKey = 1;

const withKeys = (steps: RecipeStep[]): Step[] => steps.map((s) => ({ ...s, key: nextKey++ }));
const plain = (steps: Step[]): RecipeStep[] =>
  steps.map(({ op, args, disabled }) => ({ op, ...(args && Object.keys(args).length ? { args } : {}), ...(disabled ? { disabled } : {}) }));

function parseSteps(raw: string): RecipeStep[] {
  const v: unknown = JSON.parse(raw);
  if (!Array.isArray(v)) throw new Error('A recipe is a JSON array of {"op", "args"} steps');
  return v.map((s, i) => {
    if (typeof s === 'string') return { op: s };
    if (s && typeof s === 'object' && typeof (s as RecipeStep).op === 'string') {
      const st = s as RecipeStep;
      const args = Object.fromEntries(Object.entries(st.args ?? {}).map(([k, val]) => [k, typeof val === 'string' ? val : JSON.stringify(val)]));
      return { op: st.op, args, disabled: !!st.disabled };
    }
    throw new Error(`Step ${i + 1} must be {"op": "…"}`);
  });
}

export function RecipeTool({ op, input, setInput, seed }: ToolProps) {
  const { ops } = useUtilities();
  const [steps, setSteps] = useState<Step[]>(() => withKeys(loadLocal<RecipeStep[]>(CURRENT_KEY, [])));
  const [saved, setSaved] = useState<SavedRecipe[]>(() => loadLocal<SavedRecipe[]>(SAVED_KEY, []));
  const [results, setResults] = useState<StepResult[]>([]);
  const [running, setRunning] = useState(false);
  const [tick, setTick] = useState(0);
  const [magic, setMagic] = useState<MagicResult | null>(null);
  const seq = useRef(0);

  const byId = useMemo(() => new Map(ops.map((o) => [o.id, o])), [ops]);
  const grouped = useMemo(() => {
    const m = new Map<string, OpInfo[]>();
    for (const o of ops) if (o.id !== 'recipe.run') m.set(o.category, [...(m.get(o.category) ?? []), o]);
    return [...m.entries()];
  }, [ops]);
  const hasNetwork = steps.some((s) => !s.disabled && byId.get(s.op)?.network);
  const stepsKey = JSON.stringify(plain(steps));

  useEffect(() => saveLocal(CURRENT_KEY, JSON.parse(stepsKey)), [stepsKey]);
  useSeed(seed, (a) => {
    if (!a.steps) return;
    try {
      setSteps(withKeys(parseSteps(a.steps)));
    } catch (e) {
      toast.error('Invalid recipe', errorMessage(e));
    }
  });

  // Live bake: run each step on the previous output. Network steps wait for an explicit Run.
  const manualTick = hasNetwork ? tick : 0;
  useEffect(() => {
    if (hasNetwork && manualTick === 0) {
      setResults([]);
      return;
    }
    const n = ++seq.current;
    const list = JSON.parse(stepsKey) as RecipeStep[];
    setRunning(true);
    const t = setTimeout(async () => {
      const out: StepResult[] = [];
      let cur = input;
      for (const s of list) {
        if (s.disabled) {
          out.push({ skipped: true, output: cur });
          continue;
        }
        try {
          cur = await utilsApi.run(s.op, cur, s.args ?? {});
          out.push({ output: cur });
        } catch (e) {
          out.push({ error: errorMessage(e) });
          break;
        }
        if (n !== seq.current) return;
      }
      if (n !== seq.current) return;
      setResults(out);
      setRunning(false);
    }, 180);
    return () => clearTimeout(t);
  }, [input, stepsKey, hasNetwork, manualTick]);

  const failed = results.findIndex((r) => r.error !== undefined);
  const complete = results.length === steps.length && failed < 0;
  const final = complete ? (steps.length ? (results[results.length - 1]?.output ?? '') : input) : '';

  // Background magic on the current output (CyberChef-style suggestions).
  useEffect(() => {
    const subject = steps.length ? final : input;
    if (!subject.trim() || hasNetwork || subject.length > 200_000) {
      setMagic(null);
      return;
    }
    let live = true;
    const t = setTimeout(() => {
      utilsApi
        .runJson<MagicResult>('detect.magic', subject)
        .then((r) => live && setMagic(r))
        .catch(() => live && setMagic(null));
    }, 400);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [final, input, steps.length, hasNetwork]);

  const update = (key: number, patch: Partial<Step>) => setSteps((ss) => ss.map((s) => (s.key === key ? { ...s, ...patch } : s)));
  const move = (i: number, d: -1 | 1) =>
    setSteps((ss) => {
      const j = i + d;
      if (j < 0 || j >= ss.length) return ss;
      const c = [...ss];
      [c[i], c[j]] = [c[j], c[i]];
      return c;
    });
  const add = (id: string) => {
    if (byId.has(id)) setSteps((ss) => [...ss, { key: nextKey++, op: id, args: argsFor(id) }]);
  };
  const argsFor = (id: string): UtilArgs => {
    const o = byId.get(id);
    return o ? defaultArgs(o) : {};
  };
  const appendRecipe = (r: RecipeStep[]) => setSteps((ss) => [...ss, ...withKeys(r.map((s) => ({ ...s, args: { ...argsFor(s.op), ...s.args } })))]);

  const saveRecipe = async () => {
    const name = await promptDialog({ title: 'Save recipe', label: 'Name', placeholder: 'e.g. Decode session cookie', validate: (v) => (v.trim() ? undefined : 'Enter a name') });
    if (!name) return;
    const next = [...saved.filter((s) => s.name !== name.trim()), { name: name.trim(), steps: plain(steps) }].sort((a, b) => a.name.localeCompare(b.name));
    setSaved(next);
    saveLocal(SAVED_KEY, next);
    toast.success(`Saved “${name.trim()}”`);
  };
  const deleteRecipe = async (name: string) => {
    if (!(await confirmDialog({ title: `Delete recipe “${name}”?`, message: 'It is removed from this machine’s saved recipes.', confirmText: 'Delete', danger: true }))) return;
    const next = saved.filter((s) => s.name !== name);
    setSaved(next);
    saveLocal(SAVED_KEY, next);
  };
  const importRecipe = async () => {
    const raw = await promptDialog({ title: 'Import recipe', label: 'Recipe JSON (array of {"op", "args"})', placeholder: '[{"op":"base64.decode"},{"op":"json.format"}]' });
    if (!raw) return;
    try {
      setSteps(withKeys(parseSteps(raw)));
    } catch (e) {
      toast.error('Invalid recipe', errorMessage(e));
    }
  };

  const suggestions = (magic?.candidates ?? []).filter((c) => c.score >= 50).slice(0, 3);

  return (
    <div className="page">
      <ToolHeader
        op={op}
        onExample={(inp, a) => {
          setInput(inp);
          if (a.steps) setSteps(withKeys(parseSteps(a.steps)));
        }}
      >
        {hasNetwork && <NetworkBadge />}
        <Menu
          align="right"
          trigger={(p) => (
            <button className="btn btn-sm" {...p} title="Saved recipes">
              <FolderOpen /> Recipes
            </button>
          )}
          items={() => [
            { label: 'Save current…', icon: <Save />, onSelect: saveRecipe, disabled: !steps.length },
            { label: 'Import JSON…', icon: <Import />, onSelect: importRecipe },
            { label: 'Copy as JSON', icon: <ClipboardCopy />, onSelect: () => copyText(JSON.stringify(plain(steps)), 'Recipe JSON copied'), disabled: !steps.length },
            ...(saved.length ? [{ separator: true }, { heading: 'Load' }] : []),
            ...saved.map((s) => ({ label: s.name, hint: `${s.steps.length} step${s.steps.length === 1 ? '' : 's'}`, onSelect: () => setSteps(withKeys(s.steps)) })),
            ...(saved.length ? [{ separator: true }, { heading: 'Delete' }] : []),
            ...saved.map((s) => ({ label: s.name, icon: <Trash2 />, danger: true, onSelect: () => void deleteRecipe(s.name) })),
          ]}
        />
      </ToolHeader>
      <div className="util-body">
        <div className="util-recipe">
          <div className="util-pane util-recipe-io">
            <PaneHead title="Input" meta={input ? sizeLabel(input) : undefined} />
            <div className="editor-shell">
              <CodeEditor value={input} onChange={setInput} language="plaintext" minimal />
            </div>
            <PaneHead title="Output" meta={final ? sizeLabel(final) : undefined}>
              {running && <Spinner />}
              {hasNetwork && (
                <button className="btn btn-sm btn-primary" onClick={() => setTick((t) => t + 1)} title="This recipe contains network steps; run it explicitly">
                  <Play /> Run
                </button>
              )}
              <CopyButton text={final} label="Copy" />
            </PaneHead>
            <div className="editor-shell">
              <CodeEditor value={final} language={outputLanguage(byId.get(steps[steps.length - 1]?.op ?? '') ?? op, steps[steps.length - 1]?.args ?? {}, final)} readOnly minimal />
            </div>
          </div>
          <div className="util-pane util-recipe-steps">
            <PaneHead title="Steps" meta={`${steps.length}`}>
              {steps.length > 0 && (
                <button className="btn btn-sm btn-ghost" onClick={() => setSteps([])} title="Remove all steps">
                  <X /> Clear
                </button>
              )}
            </PaneHead>
            <div className="scroll grow col-sm util-steps">
              {!steps.length && (
                <Empty icon={<ChefHat />} title="Build a recipe">
                  Chain operations: each step gets the previous step’s output. Add a step below, or apply a magic suggestion.
                </Empty>
              )}
              {steps.map((s, i) => {
                const info = byId.get(s.op);
                const r = results[i];
                return (
                  <div key={s.key} className={clsx('card util-step', s.disabled && 'off', r?.error !== undefined && 'bad')}>
                    <div className="row">
                      <span className="util-step-n">{i + 1}</span>
                      <strong className="grow truncate" title={info?.description}>
                        {info?.name ?? s.op}
                      </strong>
                      {info?.network && <NetworkBadge />}
                      <Switch checked={!s.disabled} onChange={(v) => update(s.key, { disabled: !v })} title={s.disabled ? 'Enable step' : 'Disable step (pass input through)'} />
                      <button className="icon-btn sm" onClick={() => move(i, -1)} disabled={i === 0} aria-label="Move up">
                        <ArrowUp />
                      </button>
                      <button className="icon-btn sm" onClick={() => move(i, 1)} disabled={i === steps.length - 1} aria-label="Move down">
                        <ArrowDown />
                      </button>
                      <button className="icon-btn sm" onClick={() => setSteps((ss) => ss.filter((x) => x.key !== s.key))} aria-label="Remove step">
                        <Trash2 />
                      </button>
                    </div>
                    {!info && <Callout kind="danger">Unknown operation {s.op}</Callout>}
                    {info && info.args.length > 0 && (
                      <div className="row wrap util-step-args">
                        {info.args.map((a) => (
                          <ArgField key={a.name} arg={a} value={s.args?.[a.name]} compact onChange={(v) => update(s.key, { args: { ...(s.args ?? {}), [a.name]: v } as UtilArgs })} />
                        ))}
                      </div>
                    )}
                    {r?.error !== undefined ? (
                      <div className="text-danger xs util-pre-wrap">{r.error}</div>
                    ) : r?.output !== undefined && !s.disabled ? (
                      <pre className="util-preview util-step-out" title="Output of this step">
                        {r.output.length > PREVIEW ? `${r.output.slice(0, PREVIEW)}…` : r.output || ' '}
                      </pre>
                    ) : null}
                  </div>
                );
              })}
              {failed >= 0 && failed < steps.length - 1 && <span className="subtle xs">Steps after the failing one did not run.</span>}
            </div>
            {suggestions.length > 0 && (
              <div className="util-magic-bar">
                <Wand2 size={14} />
                <span className="subtle xs">Magic:</span>
                {suggestions.map((c, i) => (
                  <button key={i} className="tag util-chip" title={`${c.reasons.join(' · ')}\n\n${c.preview}`} onClick={() => appendRecipe(c.recipe)}>
                    <Sparkles /> {c.label} <span className="subtle">{c.score.toFixed(0)}</span>
                  </button>
                ))}
              </div>
            )}
            <div className="row">
              <Plus size={14} className="subtle" />
              <select
                className="select sm grow"
                value=""
                onChange={(e) => {
                  if (e.target.value) add(e.target.value);
                }}
                aria-label="Add a step"
              >
                <option value="">Add a step…</option>
                {grouped.map(([cat, list]) => (
                  <optgroup key={cat} label={cat}>
                    {list.map((o) => (
                      <option key={o.id} value={o.id}>
                        {o.name}
                        {o.network ? ' (network)' : ''}
                      </option>
                    ))}
                  </optgroup>
                ))}
              </select>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
