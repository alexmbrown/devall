import { Fragment, useMemo, type ReactNode } from 'react';
import { marked, type Token, type Tokens, type TokensList } from 'marked';
import DOMPurify from 'dompurify';
import { openUrl } from '@tauri-apps/plugin-opener';

marked.setOptions({ gfm: true, breaks: false });

/** A fenced code block handed to `renderCodeBlock`. `index` counts fenced blocks in the source. */
export interface CodeBlockInfo {
  /** First word of the info string (```bash title → `bash`), may be empty. */
  language: string;
  code: string;
  index: number;
}

type Part = { kind: 'html'; html: string } | { kind: 'code'; block: CodeBlockInfo; html: string };

/** DOMPurify's default URI rule plus `repo:` (`repo:<name>/<path>` links into the project's repositories). */
const PURIFY = { ALLOWED_URI_REGEXP: /^(?:(?:(?:f|ht)tps?|mailto|tel|repo):|[^a-z]|[a-z+.-]+(?:[^a-z+.\-:]|$))/i };
const sanitize = (html: string) => DOMPurify.sanitize(html, PURIFY);

const render = (tokens: Token[], links: TokensList['links']) => sanitize(marked.parser(Object.assign(tokens, { links })) as string);

/** Splits the document at fenced code blocks so they can be rendered by React. */
function splitAtCodeBlocks(source: string): Part[] {
  const tokens = marked.lexer(source);
  const parts: Part[] = [];
  let pending: Token[] = [];
  let index = 0;
  const flush = () => {
    if (pending.length) parts.push({ kind: 'html', html: render(pending, tokens.links) });
    pending = [];
  };
  for (const t of tokens) {
    if (t.type === 'code' && (t as Tokens.Code).codeBlockStyle !== 'indented') {
      flush();
      const code = t as Tokens.Code;
      parts.push({ kind: 'code', block: { language: (code.lang ?? '').trim().split(/\s+/)[0] ?? '', code: code.text, index: index++ }, html: render([t], tokens.links) });
    } else pending.push(t);
  }
  flush();
  return parts;
}

/**
 * Renders Markdown (GFM) safely. Links to project files (`http/…req.yaml`, `notes/…md`,
 * `repo:<name>/<path>`) are handed to `onProjectLink`; external links open in the system browser.
 *
 * `renderCodeBlock` (optional) replaces fenced code blocks with React content (e.g. runnable
 * blocks); return `null`/`undefined` to keep the default rendering for a block.
 */
export function Markdown({
  source,
  onProjectLink,
  className,
  renderCodeBlock,
}: {
  source: string;
  onProjectLink?(path: string): void;
  className?: string;
  renderCodeBlock?(block: CodeBlockInfo): ReactNode | null | undefined;
}) {
  const html = useMemo(() => (renderCodeBlock ? '' : sanitize(marked.parse(source, { async: false }) as string)), [source, renderCodeBlock]);
  const parts = useMemo(() => (renderCodeBlock ? splitAtCodeBlocks(source) : null), [source, renderCodeBlock]);
  const onClick = (e: React.MouseEvent) => {
    const a = (e.target as HTMLElement).closest('a');
    if (!a) return;
    const href = a.getAttribute('href') ?? '';
    e.preventDefault();
    if (/^https?:\/\//i.test(href) || href.startsWith('mailto:')) openUrl(href).catch(() => {});
    else if (href) onProjectLink?.(decodeURIComponent(href.replace(/^\.?\//, '')));
  };
  if (!parts) return <div className={`markdown ${className ?? ''}`} dangerouslySetInnerHTML={{ __html: html }} onClick={onClick} />;
  return (
    <div className={`markdown ${className ?? ''}`} onClick={onClick}>
      {parts.map((p, i) => {
        if (p.kind === 'code') {
          const custom = renderCodeBlock?.(p.block);
          if (custom !== null && custom !== undefined) return <Fragment key={`c${p.block.index}`}>{custom}</Fragment>;
        }
        return <div key={i} style={{ display: 'contents' }} dangerouslySetInnerHTML={{ __html: p.html }} />;
      })}
    </div>
  );
}
