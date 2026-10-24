import { useMemo, useRef, type ClipboardEvent, type ReactNode } from 'react';
import clsx from 'clsx';
import { ArrowLeftRight, Check, CheckCircle2, CircleAlert, Minus, X } from 'lucide-react';
import { Callout, CopyButton, Spinner, Switch } from '@/components/ui/misc';
import type { CompareFinding, CompareResult, CompareSide } from '@/lib/api/utils';
import { useViewState } from '@/store/nav';
import { decodeEscapes, diffRanges, fromDisplay, lineEndingLabel, toDisplay, visibleTokens, type Range } from './compare';
import { PaneHead, ToolHeader, sizeLabel, useOpRunner, useRecordRun, useSeed, type ToolProps } from './shared';

const CHECKS: { key: keyof CompareResult['equal']; label: string; title: string }[] = [
  { key: 'exact', label: 'Exactly equal', title: 'Byte-for-byte identical' },
  { key: 'ignoreCase', label: 'Ignoring case', title: 'Unicode lower-case of both is equal' },
  { key: 'trimmed', label: 'Ignoring leading/trailing whitespace', title: 'Equal after trimming both ends' },
  { key: 'lineEndings', label: 'Ignoring line endings', title: 'CRLF, CR and LF treated alike' },
  { key: 'whitespace', label: 'Whitespace-normalized', title: 'Every run of whitespace (incl. NBSP, tabs, newlines) → one space, trimmed' },
  { key: 'nfc', label: 'Unicode NFC-equal', title: 'Canonical equivalence: é (U+00E9) = e + ◌́ (U+0065 U+0301)' },
  { key: 'nfkc', label: 'Unicode NFKC-equal', title: 'Compatibility equivalence: fullwidth Ａ = A, ﬁ = fi, ² = 2' },
  { key: 'invisibleRemoved', label: 'Ignoring invisible characters', title: 'Zero-width spaces, BOM, bidi controls removed' },
  { key: 'lookalikes', label: 'Look-alikes folded', title: 'Cyrillic/Greek/fullwidth look-alikes mapped to Latin (after NFKC)' },
  { key: 'loose', label: 'All of the above', title: 'NFKC, look-alikes, invisible characters, case and whitespace all normalized' },
];

type StatRow = { label: string; value: (s: CompareSide) => ReactNode; raw: (s: CompareSide) => unknown; title?: string };

const yesNo = (v: boolean) => (v ? 'yes' : 'no');

const STATS: StatRow[] = [
  { label: 'Characters', value: (s) => s.chars.toLocaleString(), raw: (s) => s.chars, title: 'Unicode code points' },
  { label: 'Graphemes', value: (s) => s.graphemes.toLocaleString(), raw: (s) => s.graphemes, title: 'User-perceived characters (extended grapheme clusters)' },
  { label: 'UTF-8 bytes', value: (s) => s.utf8Bytes.toLocaleString(), raw: (s) => s.utf8Bytes },
  { label: 'UTF-16 units', value: (s) => s.utf16Units.toLocaleString(), raw: (s) => s.utf16Units, title: 'What JavaScript/Java/C# string length reports' },
  { label: 'Lines', value: (s) => s.lines.toLocaleString(), raw: (s) => s.lines },
  { label: 'Line endings', value: (s) => lineEndingLabel(s.lineEndings), raw: (s) => s.lineEndings },
  { label: 'Spaces / tabs', value: (s) => `${s.whitespace.spaces} / ${s.whitespace.tabs}`, raw: (s) => `${s.whitespace.spaces}/${s.whitespace.tabs}` },
  { label: 'Other whitespace', value: (s) => s.whitespace.other, raw: (s) => s.whitespace.other, title: 'NBSP, thin space, ideographic space…' },
  { label: 'Byte order mark', value: (s) => yesNo(s.bom), raw: (s) => s.bom },
  { label: 'Trailing newline', value: (s) => yesNo(s.trailingNewline), raw: (s) => s.trailingNewline },
  { label: 'Leading / trailing space', value: (s) => `${yesNo(s.leadingWhitespace)} / ${yesNo(s.trailingWhitespace)}`, raw: (s) => `${s.leadingWhitespace}/${s.trailingWhitespace}` },
  { label: 'Non-ASCII', value: (s) => s.nonAscii.toLocaleString(), raw: (s) => s.nonAscii },
  { label: 'NFC / NFKC form', value: (s) => `${yesNo(s.isNfc)} / ${yesNo(s.isNfkc)}`, raw: (s) => `${s.isNfc}/${s.isNfkc}` },
];

/** Text with visible markers for whitespace and invisible characters. */
export function Visible({ text, className }: { text: string; className?: string }) {
  const tokens = useMemo(() => visibleTokens(text), [text]);
  return (
    <>
      {tokens.map((t, i) =>
        t.kind === 'text' ? (
          t.text
        ) : (
          <span
            key={i}
            title={t.title}
            className={clsx('mk', className, (t.kind === 'invisible' || t.kind === 'bidi' || t.kind === 'bom' || t.kind === 'control') && 'alert', (t.kind === 'nbsp' || t.kind === 'ws' || t.kind === 'cr') && 'warn')}
          >
            {t.text}
          </span>
        ),
      )}
    </>
  );
}

/** Textarea that keeps carriage returns (shown as ␍) and highlights the characters that differ. */
function CompareInput({ value, onChange, ranges, mark, placeholder, label }: { value: string; onChange(v: string): void; ranges: Range[]; mark: 'del' | 'ins'; placeholder: string; label: string }) {
  const shown = toDisplay(value);
  const backdrop = useRef<HTMLDivElement>(null);
  const segments = useMemo(() => {
    const out: ReactNode[] = [];
    let pos = 0;
    for (const [s, e] of ranges.slice(0, 5000)) {
      if (s < pos || e > shown.length) continue; // stale while typing
      if (s > pos) out.push(shown.slice(pos, s));
      out.push(
        <mark key={s} className={mark}>
          {shown.slice(s, e)}
        </mark>,
      );
      pos = e;
    }
    out.push(shown.slice(pos), '\n');
    return out;
  }, [ranges, shown, mark]);

  // Browsers turn pasted CRLF into LF; keep the clipboard's exact text so CR vs LF can be compared.
  const onPaste = (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const raw = e.clipboardData.getData('text/plain');
    if (!raw.includes('\r')) return;
    e.preventDefault();
    const el = e.currentTarget;
    const { selectionStart: start, selectionEnd: end } = el;
    const ins = toDisplay(raw);
    onChange(fromDisplay(shown.slice(0, start) + ins + shown.slice(end)));
    requestAnimationFrame(() => el.setSelectionRange(start + ins.length, start + ins.length));
  };

  return (
    <div className="util-hl util-compare-hl">
      <div className="util-hl-backdrop" ref={backdrop} aria-hidden>
        {segments}
      </div>
      <textarea
        className="util-hl-input"
        value={shown}
        spellCheck={false}
        placeholder={placeholder}
        aria-label={label}
        onChange={(e) => onChange(fromDisplay(e.target.value))}
        onPaste={onPaste}
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

function CharCell({ c }: { c: { char: string; codePoint: string; name: string | null } | null }) {
  if (!c) return <span className="subtle">(end of string)</span>;
  return (
    <span className="row-sm wrap">
      <code className="util-vis">
        <Visible text={c.char} />
      </code>
      <span className="mono xs">{c.codePoint}</span>
      <span className="xs subtle">{c.name ?? ''}</span>
    </span>
  );
}

function FindingRow({ f }: { f: CompareFinding }) {
  return (
    <tr>
      <td>
        <span className="badge outline">{f.side.toUpperCase()}</span>
      </td>
      <td className="mono xs">{f.index}</td>
      <td>
        <code className="util-vis">
          <Visible text={f.char} />
        </code>
      </td>
      <td className="mono xs">{f.codePoint}</td>
      <td className="xs">
        {f.name ?? ''}
        <div className="subtle">{f.note}</div>
      </td>
    </tr>
  );
}

/** String compare: two inputs, equality under normalizations, lengths, first difference, invisibles. */
export function CompareTool({ op, input, setInput, seed }: ToolProps) {
  const [b, setB] = useViewState('compare.b', '');
  const [escapes, setEscapes] = useViewState('compare.escapes', false);
  useSeed(seed, (a) => {
    if (a.other !== undefined) setB(a.other);
  });

  const da = escapes ? decodeEscapes(input) : { value: input };
  const db = escapes ? decodeEscapes(b) : { value: b };
  const any = input.length > 0 || b.length > 0;
  const run = useOpRunner('text.compare', da.value, { other: db.value }, { enabled: any });
  useRecordRun(op, input, { other: b }, run, { enabled: any });
  const r = useMemo<CompareResult | null>(() => {
    try {
      return run.output ? (JSON.parse(run.output) as CompareResult) : null;
    } catch {
      return null;
    }
  }, [run.output]);
  // Highlight positions only map onto the inputs when they are compared as typed.
  const ranges = useMemo(() => (r?.segments && !escapes ? diffRanges(r.segments) : { a: [], b: [] }), [r, escapes]);
  const fd = r?.firstDifference;

  return (
    <div className="page">
      <ToolHeader
        op={op}
        onExample={(inp, a) => {
          setInput(inp);
          setB(a.other ?? '');
        }}
      >
        <Switch checked={escapes} onChange={setEscapes} label={<span className="small">Escapes</span>} title={'Interpret \\n, \\r, \\t, \\uXXXX, \\u{…} and \\xNN in both inputs'} />
      </ToolHeader>
      <div className="util-body util-compare">
        <div className="util-compare-inputs">
          <div className="util-pane">
            <PaneHead title="A" meta={input ? sizeLabel(da.value) : undefined}>
              <CopyButton text={() => da.value} title="Copy A" />
            </PaneHead>
            <CompareInput value={input} onChange={setInput} ranges={ranges.a} mark="del" placeholder="First string…" label="String A" />
          </div>
          <div className="util-pane">
            <PaneHead title="B" meta={b ? sizeLabel(db.value) : undefined}>
              <button
                className="icon-btn sm"
                title="Swap A and B"
                aria-label="Swap A and B"
                onClick={() => {
                  const a = input;
                  setInput(b);
                  setB(a);
                }}
              >
                <ArrowLeftRight />
              </button>
              <CopyButton text={() => db.value} title="Copy B" />
            </PaneHead>
            <CompareInput value={b} onChange={setB} ranges={ranges.b} mark="ins" placeholder="Second string…" label="String B" />
          </div>
        </div>
        {(da.error || db.error) && <Callout kind="warning">{[da.error && `A: ${da.error}`, db.error && `B: ${db.error}`].filter(Boolean).join(' · ')}</Callout>}
        {run.error && <Callout kind="danger">{run.error}</Callout>}
        {!any ? (
          <p className="subtle small">
            Paste two strings to see whether they are really equal — and if not, exactly why: case, whitespace, line endings, Unicode normalization, zero-width or look-alike characters.
            Carriage returns from the clipboard are kept and shown as <code className="code-inline">␍</code>.
          </p>
        ) : r ? (
          <>
            <div className={clsx('util-compare-verdict', r.equal.exact ? 'eq' : 'ne')} role="status">
              {r.equal.exact ? <CheckCircle2 /> : <CircleAlert />}
              <div className="grow">
                <h3>
                  {r.equal.exact ? 'Identical' : 'Different'}
                  {fd && <span className="subtle small"> — first difference at character {fd.index.toLocaleString()} (line {fd.line}, column {fd.column})</span>}
                  {run.running && <Spinner />}
                </h3>
                {r.hints.length > 0 && (
                  <ul>
                    {r.hints.map((h) => (
                      <li key={h}>{h}</li>
                    ))}
                  </ul>
                )}
              </div>
            </div>
            <div className="util-compare-grid">
              <section className="card util-compare-card">
                <h4>Equality</h4>
                <ul className="util-checks">
                  {CHECKS.map((c) => (
                    <li key={c.key} className={r.equal[c.key] ? 'yes' : 'no'} title={c.title}>
                      {r.equal[c.key] ? <Check /> : <X />}
                      {c.label}
                    </li>
                  ))}
                </ul>
              </section>
              <section className="card util-compare-card">
                <h4>Lengths &amp; properties</h4>
                <table className="table util-stats">
                  <thead>
                    <tr>
                      <th />
                      <th>A</th>
                      <th>B</th>
                    </tr>
                  </thead>
                  <tbody>
                    {STATS.map((s) => (
                      <tr key={s.label} className={clsx(s.raw(r.a) !== s.raw(r.b) && 'differs')} title={s.title}>
                        <td className="subtle">{s.label}</td>
                        <td>{s.value(r.a)}</td>
                        <td>{s.value(r.b)}</td>
                      </tr>
                    ))}
                    <tr className={clsx(r.a.sha256 !== r.b.sha256 && 'differs')}>
                      <td className="subtle">SHA-256</td>
                      <td className="mono xs" title={r.a.sha256}>
                        <span className="row-sm">
                          <span className="truncate">{r.a.sha256}</span>
                          <CopyButton text={r.a.sha256} title="Copy SHA-256 of A" />
                        </span>
                      </td>
                      <td className="mono xs" title={r.b.sha256}>
                        <span className="row-sm">
                          <span className="truncate">{r.b.sha256}</span>
                          <CopyButton text={r.b.sha256} title="Copy SHA-256 of B" />
                        </span>
                      </td>
                    </tr>
                  </tbody>
                </table>
              </section>
              {fd && (
                <section className="card util-compare-card">
                  <h4>First difference</h4>
                  <dl className="util-firstdiff">
                    <dt>Position</dt>
                    <dd className="xs">
                      character {fd.index.toLocaleString()} · line {fd.line}, column {fd.column} · UTF-16 {fd.utf16Offset.toLocaleString()} · byte {fd.byteOffsetA.toLocaleString()}
                      {fd.byteOffsetB !== fd.byteOffsetA ? ` / ${fd.byteOffsetB.toLocaleString()}` : ''}
                    </dd>
                    <dt>A has</dt>
                    <dd>
                      <CharCell c={fd.a} />
                    </dd>
                    <dt>B has</dt>
                    <dd>
                      <CharCell c={fd.b} />
                    </dd>
                    <dt>A</dt>
                    <dd>
                      <pre className="util-vis">
                        <Visible text={fd.before} />
                        <span className="at">{fd.afterA ? <Visible text={[...fd.afterA][0]} /> : <Minus size={10} />}</span>
                        <Visible text={[...fd.afterA].slice(1).join('')} />
                      </pre>
                    </dd>
                    <dt>B</dt>
                    <dd>
                      <pre className="util-vis">
                        <Visible text={fd.before} />
                        <span className="at">{fd.afterB ? <Visible text={[...fd.afterB][0]} /> : <Minus size={10} />}</span>
                        <Visible text={[...fd.afterB].slice(1).join('')} />
                      </pre>
                    </dd>
                  </dl>
                </section>
              )}
              {!r.equal.exact && (
                <section className="card util-compare-card wide">
                  <h4>
                    Character diff <span className="badge danger">A</span>
                    <span className="badge success">B</span>
                  </h4>
                  {r.segments ? (
                    <pre className="util-vis">
                      {r.segments.map((s, i) => (
                        <span key={i} className={clsx(s.tag === 'insert' && 'ins', s.tag === 'delete' && 'del')}>
                          <Visible text={s.value} />
                        </span>
                      ))}
                    </pre>
                  ) : (
                    <span className="subtle xs">Inputs are too long for a character diff — use Text diff for large texts.</span>
                  )}
                </section>
              )}
              <section className="card util-compare-card wide">
                <h4>
                  Invisible &amp; look-alike characters
                  <span className="subtle">
                    A: {r.findingCounts.a} · B: {r.findingCounts.b}
                  </span>
                </h4>
                {r.findings.length ? (
                  <div className="util-table-wrap">
                    <table className="table">
                      <thead>
                        <tr>
                          <th>Side</th>
                          <th>Index</th>
                          <th>Shown as</th>
                          <th>Code point</th>
                          <th>Name</th>
                        </tr>
                      </thead>
                      <tbody>
                        {r.findings.map((f) => (
                          <FindingRow key={`${f.side}${f.index}`} f={f} />
                        ))}
                      </tbody>
                    </table>
                  </div>
                ) : (
                  <span className="subtle xs">None — no zero-width, bidi, BOM, control, non-breaking-space or look-alike characters in either string.</span>
                )}
              </section>
            </div>
          </>
        ) : (
          <div className="row subtle small">
            <Spinner /> Comparing…
          </div>
        )}
      </div>
    </div>
  );
}
