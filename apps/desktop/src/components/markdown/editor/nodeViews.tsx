import { useEffect, useState, type ReactNode } from 'react';
import { ImageOff } from 'lucide-react';
import type { Node as PMNode } from 'prosemirror-model';
import type { EditorView, NodeView } from 'prosemirror-view';

type ViewMutationRecord = Parameters<NonNullable<NodeView['ignoreMutation']>>[0];
import { LinkChip } from '@/components/links/LinkChip';
import { CopyButton } from '@/components/ui/misc';
import type { Portals } from './portals';

/** What node views need from the editor component (always the latest props). */
export interface ViewHost {
  portals: Portals;
  readOnly(): boolean;
  resolveImage?(src: string): Promise<string | null>;
  codeBlockExtras?(block: { language: string; code: string }): ReactNode;
  focus(): void;
}

const el = <K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  return e;
};

/** Markdown label text for display (`\[` → `[`). */
const unescape = (s: string) => s.replace(/\\([!-/:-@[-`{-~])/g, '$1');

// ── devall: chips ──────────────────────────────────────────────────────────────────────────────

class ChipView implements NodeView {
  dom: HTMLElement;
  constructor(
    private node: PMNode,
    private host: ViewHost,
  ) {
    this.dom = el('span', 'md-chip-host');
    this.render();
  }
  private render() {
    this.dom.dataset.href = this.node.attrs.href;
    this.host.portals.set(this.dom, <LinkChip href={this.node.attrs.href} label={unescape(this.node.attrs.label)} />);
  }
  update(node: PMNode) {
    if (node.type !== this.node.type) return false;
    if (node.attrs.href !== this.node.attrs.href || node.attrs.label !== this.node.attrs.label) {
      this.node = node;
      this.render();
    }
    this.node = node;
    return true;
  }
  selectNode() {
    this.dom.classList.add('ProseMirror-selectednode');
  }
  deselectNode() {
    this.dom.classList.remove('ProseMirror-selectednode');
  }
  ignoreMutation() {
    return true;
  }
  destroy() {
    this.host.portals.delete(this.dom);
  }
}

// ── images ─────────────────────────────────────────────────────────────────────────────────────

function MdImage({ src, alt, title, resolve }: { src: string; alt: string; title: string | null; resolve?(src: string): Promise<string | null> }) {
  const [url, setUrl] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    let live = true;
    if (/^(data:|blob:)/i.test(src)) setUrl(src);
    // Remote images aren't loaded (no network the user didn't ask for; the app's CSP blocks them too).
    else if (/^[a-z][a-z0-9+.-]*:/i.test(src) && !/^repo:/i.test(src)) setUrl(null);
    else if (resolve)
      resolve(src)
        .then((u) => live && setUrl(u))
        .catch(() => live && setUrl(null));
    else setUrl(null);
    return () => {
      live = false;
    };
  }, [src, resolve]);
  if (url) return <img src={url} alt={alt} title={title ?? src} draggable={false} />;
  return (
    <span className="md-image-missing" title={`${src}${title ? `\n${title}` : ''}`}>
      <ImageOff />
      <span className="truncate">{alt || src}</span>
      {url === null && /^https?:/i.test(src) && <span className="subtle">(remote image)</span>}
    </span>
  );
}

class ImageView implements NodeView {
  dom: HTMLElement;
  constructor(
    private node: PMNode,
    private host: ViewHost,
  ) {
    this.dom = el('span', 'md-image');
    this.render();
  }
  private render() {
    const { src, alt, title } = this.node.attrs;
    this.host.portals.set(this.dom, <MdImage src={src} alt={alt} title={title} resolve={this.host.resolveImage} />);
  }
  update(node: PMNode) {
    if (node.type !== this.node.type) return false;
    const changed = node.attrs.src !== this.node.attrs.src || node.attrs.alt !== this.node.attrs.alt;
    this.node = node;
    if (changed) this.render();
    return true;
  }
  selectNode() {
    this.dom.classList.add('ProseMirror-selectednode');
  }
  deselectNode() {
    this.dom.classList.remove('ProseMirror-selectednode');
  }
  ignoreMutation() {
    return true;
  }
  destroy() {
    this.host.portals.delete(this.dom);
  }
}

// ── code blocks ────────────────────────────────────────────────────────────────────────────────

const CODE_LANGUAGES = ['bash', 'sh', 'powershell', 'cmd', 'javascript', 'typescript', 'python', 'json', 'yaml', 'sql', 'graphql', 'html', 'xml', 'css', 'rust', 'go', 'java', 'csharp', 'kotlin', 'swift', 'ruby', 'php', 'dockerfile', 'ini', 'markdown', 'diff', 'text'];

/** One shared `<datalist>` of language suggestions for every code block's language box. */
function ensureLanguageList() {
  if (document.getElementById('md-code-langs')) return;
  const list = el('datalist');
  list.id = 'md-code-langs';
  for (const l of CODE_LANGUAGES) {
    const o = el('option');
    o.value = l;
    list.appendChild(o);
  }
  document.body.appendChild(list);
}

function CodeBar({ info, code, readOnly, onInfo }: { info: string; code: string; readOnly: boolean; onInfo(v: string): void }) {
  const [v, setV] = useState(info);
  useEffect(() => setV(info), [info]);
  return (
    <>
      <input
        className="md-code-lang"
        value={v}
        placeholder="language"
        list="md-code-langs"
        spellCheck={false}
        readOnly={readOnly}
        aria-label="Code block language"
        onChange={(e) => setV(e.target.value)}
        onBlur={() => v.trim() !== info && onInfo(v.trim())}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === 'Enter') {
            e.preventDefault();
            onInfo(v.trim());
          } else if (e.key === 'Escape') {
            e.preventDefault();
            setV(info);
            onInfo(info);
          }
        }}
      />
      <span className="spacer" />
      <CopyButton text={code} title="Copy code" />
    </>
  );
}

class CodeBlockView implements NodeView {
  dom: HTMLElement;
  contentDOM: HTMLElement;
  private bar: HTMLElement;
  private extra: HTMLElement;
  constructor(
    private node: PMNode,
    private view: EditorView,
    private getPos: () => number | undefined,
    private host: ViewHost,
  ) {
    ensureLanguageList();
    this.dom = el('div', 'md-code');
    this.bar = el('div', 'md-code-bar');
    this.bar.contentEditable = 'false';
    const pre = el('pre');
    this.contentDOM = el('code');
    pre.appendChild(this.contentDOM);
    this.extra = el('div', 'md-code-extra');
    this.extra.contentEditable = 'false';
    this.dom.append(this.bar, pre, this.extra);
    this.render();
  }
  private setInfo = (info: string) => {
    const pos = this.getPos();
    if (pos === undefined) return;
    if (info !== this.node.attrs.info) this.view.dispatch(this.view.state.tr.setNodeMarkup(pos, undefined, { ...this.node.attrs, info, indented: false }));
    this.host.focus();
  };
  private render() {
    const info = this.node.attrs.info as string;
    const code = this.node.textContent;
    this.dom.dataset.language = info.split(/\s/)[0] ?? '';
    this.host.portals.set(this.bar, <CodeBar info={info} code={code} readOnly={this.host.readOnly()} onInfo={this.setInfo} />);
    const extra = this.host.codeBlockExtras?.({ language: info.trim().split(/\s+/)[0] ?? '', code });
    if (extra) this.host.portals.set(this.extra, extra);
    else this.host.portals.delete(this.extra);
    this.extra.hidden = !extra;
  }
  update(node: PMNode) {
    if (node.type !== this.node.type) return false;
    const changed = node.attrs.info !== this.node.attrs.info || node.textContent !== this.node.textContent;
    this.node = node;
    if (changed) this.render();
    return true;
  }
  stopEvent(e: Event) {
    const t = e.target as Node | null;
    return !!t && (this.bar.contains(t) || this.extra.contains(t));
  }
  ignoreMutation(m: ViewMutationRecord) {
    return !this.contentDOM.contains(m.target as Node) || (m.type === 'attributes' && m.target === this.contentDOM);
  }
  destroy() {
    this.host.portals.delete(this.bar);
    this.host.portals.delete(this.extra);
  }
}

// ── raw blocks / front matter: a label above editable source ───────────────────────────────────

class RawView implements NodeView {
  dom: HTMLElement;
  contentDOM: HTMLElement;
  constructor(node: PMNode) {
    const kind = node.type.name === 'frontMatter' ? 'front' : (node.attrs.kind as string);
    this.dom = el('div', `md-raw md-raw-${kind}`);
    const label = el('div', 'md-raw-label');
    label.contentEditable = 'false';
    label.textContent = kind === 'front' ? 'Front matter (YAML)' : kind === 'html' ? 'HTML' : kind === 'def' ? 'Link definition' : 'Markdown';
    const pre = el('pre');
    this.contentDOM = el('code');
    pre.appendChild(this.contentDOM);
    this.dom.append(label, pre);
  }
  ignoreMutation(m: ViewMutationRecord) {
    return !this.contentDOM.contains(m.target as Node);
  }
}

export function nodeViews(host: ViewHost) {
  return {
    devallLink: (node: PMNode) => new ChipView(node, host),
    image: (node: PMNode) => new ImageView(node, host),
    codeBlock: (node: PMNode, view: EditorView, getPos: () => number | undefined) => new CodeBlockView(node, view, getPos, host),
    rawBlock: (node: PMNode) => new RawView(node),
    frontMatter: (node: PMNode) => new RawView(node),
  };
}
