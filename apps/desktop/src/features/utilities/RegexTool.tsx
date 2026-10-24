import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { CheckCircle2, Copy, FolderOpen, Plus, Save, Trash2, XCircle } from 'lucide-react';
import { Menu } from '@/components/ui/Menu';
import { promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { Callout, CopyButton, Segmented, Spinner, copyText } from '@/components/ui/misc';
import { api, errorMessage } from '@/lib/ipc';
import type { RegexCasesResult, RegexExplainResult, RegexMatch, RegexNode, RegexTestResult, UtilArgs } from '@/lib/api/utils';
import type { RegexEntry, RegexFile } from '@/lib/types';
import { useWorkspace } from '@/store/workspace';
import { PaneHead, ToolHeader, loadLocal, microseconds, saveLocal, sizeLabel, useOpRunner, useRecordRun, useSeed, type ToolProps } from './shared';
import { AiAssistButton } from '@/features/ai';
import { aiSamples } from './regexCases';

type Tab = 'matches' | 'replace' | 'split' | 'explain' | 'cases';
const TAB_FOR_OP: Record<string, Tab> = { 'regex.test': 'matches', 'regex.replace': 'replace', 'regex.split': 'split', 'regex.explain': 'explain', 'regex.cases': 'cases' };
const FLAGS: { f: string; title: string }[] = [
  { f: 'i', title: 'Case-insensitive' },
  { f: 'm', title: 'Multi-line: ^ and $ match at line breaks' },
  { f: 's', title: 'Dot matches newline' },
  { f: 'x', title: 'Extended: ignore whitespace, allow # comments' },
  { f: 'U', title: 'Swap greedy and lazy quantifiers' },
];
const STORE_KEY = 'devall.utils.regex';
const MAX_ROWS = 500;

interface RegexState {
  pattern: string;
  flags: string;
  flavor: 'rust' | 'pcre-like';
  replacement: string;
  /** Saved test cases (kept with the tool's local state; “Save to project” writes a file). */
  shouldMatch?: string[];
  shouldNotMatch?: string[];
  /** Project file this regex was opened from / saved to (`utilities/regex/…`). */
  savedPath?: string;
  savedName?: string;
}

function parseJson<T>(s: string): T | null {
  try {
    return s ? (JSON.parse(s) as T) : null;
  } catch {
    return null;
  }
}

/** The pattern as source code for a few languages (flags inlined where literals can't carry them). */
function snippets(p: string, flags: string): { label: string; code: string }[] {
  const inline = flags ? `(?${flags})` : '';
  const js = flags.replace(/[xU]/g, '');
  const py = [...flags].map((f) => ({ i: 're.I', m: 're.M', s: 're.S', x: 're.X' })[f as 'i']).filter(Boolean);
  const goPat = p.includes('`') ? JSON.stringify(inline + p) : `\`${inline}${p}\``;
  return [
    { label: 'JavaScript', code: `/${p.replace(/\//g, '\\/')}/g${js}` },
    { label: 'Rust', code: `Regex::new(r#"${inline}${p}"#)?` },
    { label: 'Python', code: `re.compile(${JSON.stringify(p)}${py.length ? `, ${py.join(' | ')}` : ''})` },
    { label: 'Go', code: `regexp.MustCompile(${goPat})` },
    { label: 'Java', code: `Pattern.compile(${JSON.stringify(inline.replace('U', '') + p)})` },
  ];
}

export function RegexTool({ op, input, setInput, seed }: ToolProps) {
  const [st, setSt] = useState<RegexState>(() => loadLocal<RegexState>(STORE_KEY, { pattern: '', flags: 'm', flavor: 'rust', replacement: '' }));
  const [tab, setTab] = useState<Tab>(TAB_FOR_OP[op.id] ?? 'matches');
  const [active, setActive] = useState<number | null>(null);
  const [hoverNode, setHoverNode] = useState<RegexNode | null>(null);
  const textRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => setTab(TAB_FOR_OP[op.id] ?? 'matches'), [op.id]);
  useEffect(() => saveLocal(STORE_KEY, st), [st]);
  const set = (patch: Partial<RegexState>) => setSt((s) => ({ ...s, ...patch }));
  const applyArgs = (a: UtilArgs) =>
    set({
      ...(a.pattern !== undefined && { pattern: a.pattern }),
      ...(a.flags !== undefined && { flags: a.flags }),
      ...(a.flavor === 'rust' || a.flavor === 'pcre-like' ? { flavor: a.flavor } : {}),
      ...(a.replacement !== undefined && { replacement: a.replacement }),
    });
  useSeed(seed, applyArgs);

  const base = { pattern: st.pattern, flags: st.flags, flavor: st.flavor };
  const hasPattern = st.pattern.length > 0;
  const test = useOpRunner('regex.test', input, base, { enabled: hasPattern, delay: 90 });
  const explain = useOpRunner('regex.explain', st.pattern, { flags: st.flags, flavor: st.flavor }, { enabled: hasPattern && tab === 'explain' });
  const replaced = useOpRunner('regex.replace', input, { ...base, replacement: st.replacement }, { enabled: hasPattern && tab === 'replace' });
  const split = useOpRunner('regex.split', input, base, { enabled: hasPattern && tab === 'split' });
  const shouldMatch = useMemo(() => st.shouldMatch ?? [], [st.shouldMatch]);
  const shouldNotMatch = useMemo(() => st.shouldNotMatch ?? [], [st.shouldNotMatch]);
  const caseCount = shouldMatch.length + shouldNotMatch.length;
  const casesInput = useMemo(() => JSON.stringify({ shouldMatch, shouldNotMatch }), [shouldMatch, shouldNotMatch]);
  const cases = useOpRunner('regex.cases', casesInput, base, { enabled: hasPattern && caseCount > 0, delay: 120 });
  const caseResult = useMemo(() => parseJson<RegexCasesResult>(cases.output), [cases.output]);
  // Utility history: the pattern (and replacement) with the test text.
  const primary = op.id === 'regex.replace' ? replaced : op.id === 'regex.split' ? split : op.id === 'regex.explain' ? explain : test;
  useRecordRun(op, input, op.id === 'regex.replace' ? { ...base, replacement: st.replacement } : base, primary, { enabled: hasPattern && op.id !== 'regex.cases' });

  const result = useMemo(() => parseJson<RegexTestResult>(test.output), [test.output]);
  const explained = useMemo(() => parseJson<RegexExplainResult>(explain.output), [explain.output]);
  const parts = useMemo(() => parseJson<string[]>(split.output), [split.output]);
  const matches = hasPattern && !test.error ? (result?.matches ?? []) : [];

  const selectMatch = (m: RegexMatch) => {
    const el = textRef.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(m.start, m.end);
  };

  const toggleFlag = (f: string) => set({ flags: st.flags.includes(f) ? st.flags.replace(f, '') : st.flags + f });

  return (
    <div className="page">
      <ToolHeader
        op={op}
        onExample={(inp, a) => {
          if (op.id === 'regex.explain') set({ pattern: inp, flags: a.flags ?? '', flavor: a.flavor === 'pcre-like' ? 'pcre-like' : 'rust' });
          else if (op.id === 'regex.cases') {
            const c = parseJson<{ shouldMatch?: string[]; shouldNotMatch?: string[] }>(inp);
            set({ flags: '', flavor: 'rust', shouldMatch: c?.shouldMatch ?? [], shouldNotMatch: c?.shouldNotMatch ?? [] });
            applyArgs(a);
          } else {
            setInput(inp);
            set({ flags: '', flavor: 'rust', replacement: '' });
            applyArgs(a);
          }
        }}
      />
      <div className="util-body">
        <div className="util-regex-bar">
          <span className="util-regex-delim">/</span>
          <input
            className="util-regex-input mono"
            value={st.pattern}
            placeholder="pattern, e.g. (?<year>\d{4})-(\d{2})"
            spellCheck={false}
            autoFocus
            onChange={(e) => set({ pattern: e.target.value })}
            aria-label="Regular expression"
          />
          <span className="util-regex-delim">/</span>
          <AiAssistButton
            feature="nlRegex"
            compact
            title="Describe a pattern in plain words (the test text and test cases are used as samples)"
            input={(d) => ({ description: d, ...aiSamples(input, shouldMatch, shouldNotMatch) })}
            onResult={(r) => set({ pattern: r.pattern, flags: r.flags || st.flags })}
            applyLabel="Use pattern"
          />
          <div className="util-flags" role="group" aria-label="Flags">
            {FLAGS.map(({ f, title }) => (
              <button key={f} className={clsx('util-flag', st.flags.includes(f) && 'on')} title={title} onClick={() => toggleFlag(f)} aria-pressed={st.flags.includes(f)}>
                {f}
              </button>
            ))}
          </div>
          <Segmented
            value={st.flavor}
            onChange={(v) => set({ flavor: v })}
            options={[
              { value: 'rust', label: 'Rust', title: 'regex crate: linear time, no look-around or backreferences' },
              { value: 'pcre-like', label: 'PCRE / JS', title: 'fancy-regex: look-around, backreferences, atomic groups (backtracking, guarded)' },
            ]}
          />
          <Menu
            align="right"
            trigger={(p) => (
              <button className="btn btn-sm" {...p} disabled={!hasPattern} title="Copy the pattern as code">
                <Copy /> Code
              </button>
            )}
            items={() => snippets(st.pattern, st.flags).map((s) => ({ label: s.label, hint: s.code.length > 40 ? `${s.code.slice(0, 40)}…` : s.code, onSelect: () => copyText(s.code, `Copied ${s.label} snippet`) }))}
          />
        </div>
        <div className="util-regex-status row">
          {!hasPattern ? (
            <span className="subtle xs">Type a pattern. Matches highlight live as you edit the text below.</span>
          ) : test.error ? (
            <span className="text-danger xs util-pre-wrap">{test.error}</span>
          ) : result ? (
            <>
              <span className={clsx('badge', result.matchCount ? 'success' : 'outline')}>
                {result.matchCount}
                {result.truncated ? '+' : ''} match{result.matchCount === 1 ? '' : 'es'}
              </span>
              <span className="subtle xs">
                {result.groupCount} group{result.groupCount === 1 ? '' : 's'} · compile {microseconds(result.compileMicros)} · match {microseconds(result.matchMicros)}
              </span>
              {result.error && <span className="badge danger" title={result.error}>aborted</span>}
              {caseResult && caseCount > 0 && (
                <button className={clsx('badge', caseResult.failed ? 'danger' : 'success')} onClick={() => setTab('cases')} title="Test cases">
                  {caseResult.passed}/{caseCount} cases pass
                </button>
              )}
            </>
          ) : null}
          {test.running && <Spinner />}
        </div>
        {result?.error && <Callout kind="danger">{result.error}</Callout>}
        {result?.warnings.map((w) => (
          <Callout key={w} kind="warning">
            {w}
          </Callout>
        ))}
        <div className="util-regex-main">
          <div className="util-pane">
            <PaneHead title="Test text" meta={input ? sizeLabel(input) : undefined} />
            <HighlightedText value={input} onChange={setInput} matches={matches} active={active} textRef={textRef} />
          </div>
          <div className="util-pane">
            <PaneHead
              lead={
                <Segmented
                  className="util-regex-tabs"
                  value={tab}
                  onChange={setTab}
                  options={[
                    { value: 'matches', label: `Matches${result && hasPattern ? ` (${result.matchCount})` : ''}` },
                    { value: 'replace', label: 'Replace' },
                    { value: 'split', label: 'Split' },
                    { value: 'explain', label: 'Explain' },
                    { value: 'cases', label: `Tests${caseCount ? ` (${caseResult && hasPattern ? `${caseResult.passed}/${caseCount}` : caseCount})` : ''}` },
                  ]}
                />
              }
            />
            <div className="util-regex-panel">
              {tab === 'matches' && <MatchTable result={hasPattern ? result : null} onHover={setActive} onPick={selectMatch} />}
              {tab === 'replace' && (
                <div className="col-sm h-full">
                  <input
                    className="input sm mono"
                    value={st.replacement}
                    placeholder="Replacement — $1, ${name}, $$ for a literal $"
                    onChange={(e) => set({ replacement: e.target.value })}
                    spellCheck={false}
                  />
                  {replaced.error ? (
                    <Callout kind="danger">{replaced.error}</Callout>
                  ) : (
                    <>
                      <PaneHead title="Result">
                        <CopyButton text={replaced.output} label="Copy" />
                      </PaneHead>
                      <pre className="util-preview grow">{hasPattern ? replaced.output : input}</pre>
                    </>
                  )}
                </div>
              )}
              {tab === 'split' &&
                (split.error ? (
                  <Callout kind="danger">{split.error}</Callout>
                ) : (
                  <div className="col-sm">
                    <PaneHead title={parts ? `${parts.length} part${parts.length === 1 ? '' : 's'}` : 'Parts'}>
                      <CopyButton text={(parts ?? []).join('\n')} label="Copy lines" />
                    </PaneHead>
                    <ol className="util-split">
                      {(parts ?? []).slice(0, MAX_ROWS).map((p, i) => (
                        <li key={i}>
                          <code className="mono">{p === '' ? <span className="subtle">(empty)</span> : p}</code>
                        </li>
                      ))}
                    </ol>
                  </div>
                ))}
              {tab === 'cases' && (
                <CasesPanel
                  shouldMatch={shouldMatch}
                  shouldNotMatch={shouldNotMatch}
                  result={hasPattern && !cases.error ? caseResult : null}
                  error={hasPattern ? cases.error : null}
                  onChange={(m, n) => set({ shouldMatch: m, shouldNotMatch: n })}
                  st={st}
                  set={set}
                />
              )}
              {tab === 'explain' &&
                (explain.error ? (
                  <Callout kind="danger">
                    <span className="util-pre-wrap">{explain.error}</span>
                  </Callout>
                ) : explained ? (
                  <div className="col-sm">
                    <PatternPreview pattern={st.pattern} node={hoverNode} />
                    {explained.warnings.map((w) => (
                      <span key={w} className="subtle xs">
                        {w}
                      </span>
                    ))}
                    <ExplainTree nodes={explained.nodes} onHover={setHoverNode} />
                  </div>
                ) : (
                  <span className="subtle xs">Type a pattern to see it explained token by token.</span>
                ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

/** Should-match / should-not-match strings with live pass/fail, plus saving to the project. */
function CasesPanel({
  shouldMatch,
  shouldNotMatch,
  result,
  error,
  onChange,
  st,
  set,
}: {
  shouldMatch: string[];
  shouldNotMatch: string[];
  result: RegexCasesResult | null;
  error: string | null;
  onChange(shouldMatch: string[], shouldNotMatch: string[]): void;
  st: RegexState;
  set(patch: Partial<RegexState>): void;
}) {
  const project = useWorkspace((s) => s.project);
  const [draft, setDraft] = useState({ match: '', noMatch: '' });
  const [saved, setSaved] = useState<RegexEntry[] | null>(null);
  const root = project?.root;
  const reloadSaved = useCallback(() => {
    if (root) api.regexList(root).then(setSaved, () => setSaved([]));
  }, [root]);
  useEffect(reloadSaved, [reloadSaved]);
  const outcome = (text: string, should: boolean) => result?.cases.find((c) => c.text === text && c.shouldMatch === should);

  const save = async () => {
    if (!project) return;
    const name = st.savedName ?? (await promptDialog({ title: 'Save regex to the project', label: 'Name', placeholder: 'Order id', validate: (v) => (v.trim() ? undefined : 'Required') }));
    if (!name) return;
    const file: RegexFile = {
      name,
      pattern: st.pattern,
      ...(st.flags ? { flags: st.flags } : {}),
      ...(st.flavor !== 'rust' ? { flavor: st.flavor } : {}),
      ...(st.replacement ? { replacement: st.replacement } : {}),
      ...(shouldMatch.length ? { shouldMatch } : {}),
      ...(shouldNotMatch.length ? { shouldNotMatch } : {}),
    };
    try {
      let existing: RegexFile | null = null;
      if (st.savedPath) existing = await api.regexRead(project.root, st.savedPath).catch(() => null);
      const path = await api.regexSave(project.root, existing ? st.savedPath! : null, existing ? { ...existing, ...file, description: existing.description } : file);
      set({ savedPath: path, savedName: name });
      reloadSaved();
      toast.success('Saved', `utilities/regex/${path} — commit it so the team shares the pattern and its tests.`);
    } catch (e) {
      toast.error('Could not save', errorMessage(e));
    }
  };

  const openSaved = async (path: string) => {
    if (!project) return;
    try {
      const f = await api.regexRead(project.root, path);
      set({
        pattern: f.pattern,
        flags: f.flags ?? '',
        flavor: f.flavor === 'pcre-like' ? 'pcre-like' : 'rust',
        replacement: f.replacement ?? '',
        shouldMatch: f.shouldMatch ?? [],
        shouldNotMatch: f.shouldNotMatch ?? [],
        savedPath: path,
        savedName: f.name,
      });
    } catch (e) {
      toast.error('Could not open', errorMessage(e));
    }
  };

  const list = (items: string[], should: boolean) => (
    <div className="col-sm">
      {items.map((text, i) => {
        const o = outcome(text, should);
        return (
          <div key={`${i}-${text}`} className={clsx('util-case', o && (o.passed ? 'pass' : 'fail'))}>
            {o ? o.passed ? <CheckCircle2 size={14} className="text-success" /> : <XCircle size={14} className="text-danger" /> : <span className="util-case-dot" />}
            <code className="mono grow util-cell" title={o?.match ? `matched “${o.match}”` : o?.error ?? undefined}>
              {text === '' ? <span className="subtle">(empty)</span> : text}
            </code>
            {o?.match != null && o.match !== text && <span className="subtle xs truncate" style={{ maxWidth: 140 }}>“{o.match}”</span>}
            <button
              className="icon-btn sm"
              aria-label="Remove case"
              onClick={() => (should ? onChange(items.filter((_, j) => j !== i), shouldNotMatch) : onChange(shouldMatch, items.filter((_, j) => j !== i)))}
            >
              <Trash2 />
            </button>
          </div>
        );
      })}
      <form
        className="row-sm"
        onSubmit={(e) => {
          e.preventDefault();
          const v = should ? draft.match : draft.noMatch;
          if (!v) return;
          if (should) onChange([...shouldMatch, v], shouldNotMatch);
          else onChange(shouldMatch, [...shouldNotMatch, v]);
          setDraft((d) => (should ? { ...d, match: '' } : { ...d, noMatch: '' }));
        }}
      >
        <input
          className="input sm mono grow"
          value={should ? draft.match : draft.noMatch}
          placeholder={should ? 'a string the pattern must match' : 'a string it must not match'}
          onChange={(e) => setDraft((d) => (should ? { ...d, match: e.target.value } : { ...d, noMatch: e.target.value }))}
          spellCheck={false}
        />
        <button className="btn btn-sm" type="submit">
          <Plus /> Add
        </button>
      </form>
    </div>
  );

  return (
    <div className="col">
      <div className="row-sm wrap">
        {result ? (
          <span className={clsx('badge', result.failed ? 'danger' : 'success')}>
            {result.passed}/{result.passed + result.failed} pass
          </span>
        ) : (
          <span className="subtle xs">A case passes when the pattern finds a match in it (anchor with ^…$ for whole strings).</span>
        )}
        <span className="spacer" />
        {st.savedPath && <span className="mono xs subtle truncate" title={`utilities/regex/${st.savedPath}`}>{st.savedPath}</span>}
        <Menu
          align="right"
          items={() =>
            !project
              ? [{ label: 'Open a project to use saved regexes', disabled: true }]
              : saved?.length
                ? saved.map((r) => ({ label: r.name, hint: r.error ? 'invalid file' : `${r.cases} case${r.cases === 1 ? '' : 's'}`, disabled: !!r.error, onSelect: () => openSaved(r.path) }))
                : [{ label: saved ? 'No saved regexes in utilities/regex/' : 'Loading…', disabled: true }]
          }
          trigger={(p) => (
            <button
              className="btn btn-sm btn-ghost"
              {...p}
              onClick={(e) => {
                reloadSaved();
                p.onClick(e);
              }}
              title="Open a regex saved in this project"
            >
              <FolderOpen /> Open
            </button>
          )}
        />
        <button className="btn btn-sm" disabled={!project || !st.pattern} onClick={save} title={project ? 'Save the pattern and its test cases to utilities/regex/ in this project' : 'Open a project to save regexes into it'}>
          <Save /> {st.savedPath ? 'Save' : 'Save to project'}
        </button>
      </div>
      {error && <Callout kind="danger">{error}</Callout>}
      <span className="label">Should match</span>
      {list(shouldMatch, true)}
      <span className="label">Should not match</span>
      {list(shouldNotMatch, false)}
    </div>
  );
}

/** A textarea over a backdrop that paints the match ranges (UTF-16 offsets from the backend). */
function HighlightedText({ value, onChange, matches, active, textRef }: { value: string; onChange(v: string): void; matches: RegexMatch[]; active: number | null; textRef: React.RefObject<HTMLTextAreaElement | null> }) {
  const backdrop = useRef<HTMLDivElement>(null);
  const segments = useMemo(() => {
    const out: ReactNode[] = [];
    let pos = 0;
    for (const m of matches.slice(0, 5000)) {
      if (m.start < pos || m.end > value.length) continue; // stale result while typing
      if (m.start > pos) out.push(value.slice(pos, m.start));
      out.push(
        <mark key={m.index} className={clsx(m.index % 2 ? 'b' : 'a', m.start === m.end && 'empty', active === m.index && 'on')}>
          {value.slice(m.start, m.end)}
        </mark>,
      );
      pos = m.end;
    }
    out.push(value.slice(pos));
    out.push('\n');
    return out;
  }, [matches, value, active]);
  return (
    <div className="util-hl">
      <div className="util-hl-backdrop" ref={backdrop} aria-hidden>
        {segments}
      </div>
      <textarea
        ref={textRef}
        className="util-hl-input"
        value={value}
        spellCheck={false}
        placeholder="Paste text to test against…"
        onChange={(e) => onChange(e.target.value)}
        onScroll={(e) => {
          if (backdrop.current) {
            backdrop.current.scrollTop = e.currentTarget.scrollTop;
            backdrop.current.scrollLeft = e.currentTarget.scrollLeft;
          }
        }}
      />
    </div>
  );
}

function MatchTable({ result, onHover, onPick }: { result: RegexTestResult | null; onHover(i: number | null): void; onPick(m: RegexMatch): void }) {
  if (!result) return <span className="subtle xs">Matches, spans and capture groups appear here.</span>;
  if (!result.matches.length) return <span className="subtle xs">No matches.</span>;
  const groups = result.groupNames;
  return (
    <div className="util-table-wrap" onMouseLeave={() => onHover(null)}>
      <table className="table util-match-table">
        <thead>
          <tr>
            <th>#</th>
            <th>Match</th>
            <th>Span</th>
            {groups.map((g) => (
              <th key={g.index} title={`Group ${g.index}`}>
                {g.name ?? `$${g.index}`}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {result.matches.slice(0, MAX_ROWS).map((m) => (
            <tr key={m.index} className="clickable" onMouseEnter={() => onHover(m.index)} onClick={() => onPick(m)} title="Click to select in the text">
              <td className="subtle">{m.index + 1}</td>
              <td className="mono util-cell">{m.text === '' ? <span className="subtle">(empty)</span> : m.text}</td>
              <td className="mono subtle nowrap">
                {m.start}–{m.end}
              </td>
              {m.groups.map((g) =>
                g.matched === false ? (
                  <td key={g.index} className="subtle">
                    —
                  </td>
                ) : (
                  <td key={g.index} className="mono util-cell" title={`${g.start}–${g.end}`}>
                    {g.text}
                  </td>
                ),
              )}
            </tr>
          ))}
        </tbody>
      </table>
      {result.matches.length > MAX_ROWS && <div className="subtle xs p-2">Showing the first {MAX_ROWS} matches.</div>}
    </div>
  );
}

function PatternPreview({ pattern, node }: { pattern: string; node: RegexNode | null }) {
  const s = node?.start;
  const e = node?.end;
  return (
    <div className="util-pattern-preview mono">
      {s !== undefined && e !== undefined ? (
        <>
          {pattern.slice(0, s)}
          <mark>{pattern.slice(s, e)}</mark>
          {pattern.slice(e)}
        </>
      ) : (
        pattern
      )}
    </div>
  );
}

function ExplainTree({ nodes, onHover, depth = 0 }: { nodes: RegexNode[]; onHover(n: RegexNode | null): void; depth?: number }) {
  return (
    <ul className={clsx('util-explain', depth === 0 && 'root')} onMouseLeave={depth === 0 ? () => onHover(null) : undefined}>
      {nodes.map((n, i) => (
        <li key={i}>
          <div className={clsx('util-explain-row', `k-${n.kind}`)} onMouseEnter={() => onHover(n)}>
            <code className="mono">{n.text || '∅'}</code>
            <span>{n.description}</span>
          </div>
          {n.children && n.children.length > 0 && <ExplainTree nodes={n.children} onHover={onHover} depth={depth + 1} />}
        </li>
      ))}
    </ul>
  );
}
