import { useMemo, useState } from 'react';
import { CodeEditor, languageFor } from '@/components/editor/CodeEditor';
import { CopyButton, Segmented, formatBytes } from '@/features/mocks/components/ui';

/** Read-only body viewer with pretty/raw toggle. */
export function BodyView({ body, base64, contentType, maxHeight = 420 }: { body?: string; base64?: string; contentType?: string; maxHeight?: number }) {
  const [mode, setMode] = useState<'pretty' | 'raw'>('pretty');
  const text = body ?? (base64 ? safeAtob(base64) : '');
  const lang = languageFor(contentType, text);
  const pretty = useMemo(() => {
    if (mode === 'raw') return text;
    if (lang === 'json') {
      try {
        return JSON.stringify(JSON.parse(text), null, 2);
      } catch {
        return text;
      }
    }
    if (lang === 'xml') return formatXml(text);
    return text;
  }, [text, lang, mode]);
  if (!text) return <div className="subtle small">(empty body)</div>;
  const binary = !body && !!base64 && /[\x00-\x08\x0e-\x1f]/.test(text);
  return (
    <div className="col-sm">
      <div className="row-sm">
        <span className="subtle xs grow">
          {contentType || 'no content-type'} · {formatBytes(new Blob([text]).size)}
        </span>
        <Segmented value={mode} onChange={setMode} options={[{ value: 'pretty', label: 'Pretty' }, { value: 'raw', label: 'Raw' }]} />
        <CopyButton text={pretty} />
      </div>
      {binary ? (
        <pre className="code">{`<binary, ${formatBytes(text.length)}> base64:\n${base64?.slice(0, 2000)}`}</pre>
      ) : text.length > 200_000 ? (
        <pre className="code" style={{ maxHeight }}>{pretty.slice(0, 200_000)}…</pre>
      ) : (
        <div className="editor-shell">
          <CodeEditor value={pretty} readOnly language={lang} autoHeight={{ min: 40, max: maxHeight }} minimal />
        </div>
      )}
    </div>
  );
}

function safeAtob(s: string) {
  try {
    return atob(s);
  } catch {
    return s;
  }
}

export function formatXml(xml: string): string {
  try {
    let indent = 0;
    return xml
      .replace(/>\s*</g, '><')
      .replace(/</g, '\n<')
      .split('\n')
      .filter(Boolean)
      .map((line) => {
        if (/^<\//.test(line)) indent = Math.max(0, indent - 1);
        const out = '  '.repeat(indent) + line;
        if (/^<[^!?/][^>]*[^/]>$/.test(line) && !/<\/[^>]+>$/.test(line)) indent++;
        return out;
      })
      .join('\n')
      .replace(/>\n\s*([^<\s][^<]*)\n\s*<\//g, '>$1</');
  } catch {
    return xml;
  }
}

export function HeadersTable({ headers }: { headers?: Record<string, string | string[]> }) {
  const entries = Object.entries(headers ?? {});
  if (!entries.length) return <div className="subtle small">(none)</div>;
  return (
    <dl className="kv mono">
      {entries.map(([k, v]) => (
        <div key={k} style={{ display: 'contents' }}>
          <dt>{k}</dt>
          <dd>{Array.isArray(v) ? v.join(', ') : v}</dd>
        </div>
      ))}
    </dl>
  );
}
