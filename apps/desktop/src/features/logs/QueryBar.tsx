import { useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { CircleHelp, CornerDownLeft, Search, X } from 'lucide-react';
import { Kbd } from '@/components/ui/misc';
import { logsApi, type QueryError } from '@/lib/api/logs';

const EXAMPLES: [string, string][] = [
  ['timeout', 'Free text, case-insensitive, anywhere in the entry'],
  ['"connection reset"', 'Exact phrase'],
  ['/time(d)?out/i', 'Regex over the raw entry'],
  ['status>=500', 'Field comparison (numeric when numbers)'],
  ['user.id=42', 'Dotted paths into JSON'],
  ['path=/api/*', 'Glob; field=* means "has the field"'],
  ['msg~"refused|reset"', 'Regex on a field; !~ negates'],
  ['host:web', 'Field contains (case-insensitive)'],
  ['level>=warn', 'trace < debug < info < warn < error < fatal'],
  ['ts>=now-15m', 'Time: now-15m, -1h, 2024-01-02T10:00, epoch ms'],
  ['trace=4bf92f35', 'Correlation / trace id'],
  ['error -health', 'AND is implicit; -term or NOT term negates'],
  ['(timeout OR refused) AND status>=500', 'OR / || and grouping'],
];

interface Props {
  applied: string;
  onApply(query: string): void;
  inputRef?: React.Ref<HTMLInputElement>;
  placeholder?: string;
}

/** Query input: validates as you type, applies on Enter, Esc reverts/clears, `?` for syntax. */
export function QueryBar({ applied, onApply, inputRef, placeholder }: Props) {
  const [draft, setDraft] = useState(applied);
  const [error, setError] = useState<QueryError | null>(null);
  const [help, setHelp] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);

  useEffect(() => setDraft(applied), [applied]);

  useEffect(() => {
    let alive = true;
    const t = setTimeout(() => {
      logsApi
        .checkQuery(draft)
        .then((e) => alive && setError(e))
        .catch(() => alive && setError(null));
    }, 120);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [draft]);

  useEffect(() => {
    if (!help) return;
    const close = (e: MouseEvent) => {
      if (!wrap.current?.contains(e.target as Node)) setHelp(false);
    };
    window.addEventListener('mousedown', close);
    return () => window.removeEventListener('mousedown', close);
  }, [help]);

  const apply = (q = draft) => {
    if (error && q === draft) return;
    onApply(q.trim());
  };
  const dirty = draft.trim() !== applied.trim();
  const chars = [...draft];

  return (
    <div className="logs-query" ref={wrap}>
      <div className={clsx('input-group logs-query-input', error && 'invalid')}>
        <Search />
        <input
          ref={inputRef}
          className={clsx('input sm mono', error && 'invalid')}
          value={draft}
          placeholder={placeholder ?? 'Search… e.g. timeout status>=500 -health   (press / to focus)'}
          spellCheck={false}
          aria-invalid={!!error}
          aria-label="Log query"
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              apply();
            } else if (e.key === 'Escape') {
              e.preventDefault();
              if (dirty) setDraft(applied);
              else if (draft) {
                setDraft('');
                onApply('');
              } else (e.target as HTMLInputElement).blur();
            }
          }}
        />
        {dirty && !error && (
          <span className="input-suffix apply-hint" title="Press Enter to apply">
            <CornerDownLeft size={12} />
          </span>
        )}
        {!dirty && draft && (
          <button className="icon-btn sm input-suffix" onClick={() => (setDraft(''), onApply(''))} aria-label="Clear query">
            <X />
          </button>
        )}
      </div>
      <button className={clsx('icon-btn', help && 'on')} onClick={() => setHelp((h) => !h)} title="Query syntax" aria-label="Query syntax" aria-expanded={help}>
        <CircleHelp />
      </button>
      {error && (
        <div className="logs-query-error" role="alert">
          <code>
            {chars.slice(0, error.start).join('')}
            <mark>{chars.slice(error.start, Math.max(error.end, error.start + 1)).join('') || ' '}</mark>
            {chars.slice(Math.max(error.end, error.start + 1)).join('')}
          </code>
          <span>{error.message}</span>
        </div>
      )}
      {help && (
        <div className="logs-help menu" role="dialog" aria-label="Query syntax">
          <div className="menu-label">Query syntax — click an example to use it</div>
          <table className="table">
            <tbody>
              {EXAMPLES.map(([q, d]) => (
                <tr
                  key={q}
                  className="clickable"
                  onClick={() => {
                    setDraft(q);
                    onApply(q);
                    setHelp(false);
                  }}
                >
                  <td>
                    <code className="code-inline">{q}</code>
                  </td>
                  <td className="subtle">{d}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="logs-help-foot subtle">
            Special fields: <code>level msg ts raw line source format trace</code>. Keywords <code>AND OR NOT</code> are upper-case. <Kbd>Enter</Kbd> apply ·{' '}
            <Kbd>Esc</Kbd> revert · in the table <Kbd>j</Kbd>/<Kbd>k</Kbd> move, <Kbd>Space</Kbd> expand, <Kbd>Enter</Kbd> details, <Kbd>c</Kbd> copy.
          </div>
        </div>
      )}
    </div>
  );
}
