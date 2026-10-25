import { Fragment, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { marked, type Token, type Tokens, type TokensList } from 'marked';
import DOMPurify from 'dompurify';
import { openUrl } from '@tauri-apps/plugin-opener';
import { isDevallLink } from '@/lib/links/syntax';
import { LinkChip, useFollowLink } from '@/components/links/LinkChip';

marked.setOptions({ gfm: true, breaks: false });

/** A fenced code block handed to `renderCodeBlock`. `index` counts fenced blocks in the source. */
export interface CodeBlockInfo {
  /** First word of the info string (```bash title → `bash`), may be empty. */
  language: string;
  code: string;
  index: number;
}

type Part = { kind: 'html'; html: string } | { kind: 'code'; block: CodeBlockInfo; html: string };

/**
 * DOMPurify's default URI rule plus `repo:` (`repo:<name>/<path>` links into the project's
 * repositories) and `devall:` (links to /dev/all items, which only ever navigate inside the app).
 */
const PURIFY = { ALLOWED_URI_REGEXP: /^(?:(?:(?:f|ht)tps?|mailto|tel|repo|devall):|[^a-z]|[a-z+.-]+(?:[^a-z+.\-:]|$))/i };
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

/** A rendered `devall:` anchor: its Markdown text (the fallback title) moves into a data attribute, a live chip replaces it. */
interface ChipSlot {
  el: HTMLAnchorElement;
  href: string;
  label: string;
}

const devallHref = (a: Element | null) => {
  const href = a?.getAttribute('href') ?? '';
  return isDevallLink(href) ? href : null;
};

/**
 * Renders Markdown (GFM) safely. `devall:` links become live chips (icon, current title) that open
 * the item inside the app — Ctrl/Cmd+click or middle-click to the side; they never navigate the
 * webview or reach the browser. Links to project files (`http/…req.yaml`, `notes/…md`,
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
  const host = useRef<HTMLDivElement>(null);
  const [chips, setChips] = useState<ChipSlot[]>([]);
  const follow = useFollowLink();

  // After each render of new HTML: find its devall: anchors and give each a chip (portal).
  useLayoutEffect(() => {
    const root = host.current;
    if (!root) return;
    const found: ChipSlot[] = [];
    for (const a of root.querySelectorAll<HTMLAnchorElement>('a[href]')) {
      const href = devallHref(a);
      if (!href) continue;
      // Kept in an attribute so a second pass (StrictMode, re-render) still knows the text.
      if (a.dataset.dlLabel === undefined) {
        a.dataset.dlLabel = a.textContent ?? '';
        a.textContent = '';
        a.classList.add('dl-anchor');
      }
      found.push({ el: a, href, label: a.dataset.dlLabel });
    }
    setChips((prev) => (prev.length === found.length && prev.every((c, i) => c.el === found[i].el && c.href === found[i].href) ? prev : found));
  }, [html, parts]);

  const onClick = (e: React.MouseEvent) => {
    const a = (e.target as HTMLElement).closest('a');
    if (!a) return;
    const href = a.getAttribute('href') ?? '';
    e.preventDefault();
    if (isDevallLink(href)) follow(href, e);
    else if (/^https?:\/\//i.test(href) || href.startsWith('mailto:')) openUrl(href).catch(() => {});
    else if (href) onProjectLink?.(decodeURIComponent(href.replace(/^\.?\//, '')));
  };
  // Middle click on a devall: link opens it to the side (and must not start autoscroll or a new window).
  const onAux = (e: React.MouseEvent) => {
    const href = devallHref((e.target as HTMLElement).closest('a'));
    if (!href || e.button !== 1) return;
    e.preventDefault();
    follow(href, { ctrlKey: false, metaKey: false, button: 1 });
  };
  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button === 1 && devallHref((e.target as HTMLElement).closest('a'))) e.preventDefault();
  };
  // The chips render through portals into the anchors; as children of the container their clicks
  // bubble (in React's tree) to the handlers above.
  const portals = chips.map((c, i) => createPortal(<LinkChip href={c.href} label={c.label} />, c.el, `chip-${i}`));
  return (
    <div ref={host} className={`markdown ${className ?? ''}`} onClick={onClick} onAuxClick={onAux} onMouseDown={onMouseDown}>
      {parts ? (
        parts.map((p, i) => {
          if (p.kind === 'code') {
            const custom = renderCodeBlock?.(p.block);
            if (custom !== null && custom !== undefined) return <Fragment key={`c${p.block.index}`}>{custom}</Fragment>;
          }
          return <div key={i} style={{ display: 'contents' }} dangerouslySetInnerHTML={{ __html: p.html }} />;
        })
      ) : (
        <div style={{ display: 'contents' }} dangerouslySetInnerHTML={{ __html: html }} />
      )}
      {portals}
    </div>
  );
}
