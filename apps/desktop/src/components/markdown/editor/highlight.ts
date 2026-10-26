import type { Node as PMNode } from 'prosemirror-model';
import { Plugin, PluginKey } from 'prosemirror-state';
import { Decoration, DecorationSet, type EditorView } from 'prosemirror-view';
import { N } from '../schema';

/**
 * Syntax highlighting for code blocks with the highlighter the app already bundles: Monaco's
 * `colorize` (its tokenizers and the current Monaco theme's colours, so code looks the same as in
 * the code editors). Monaco is loaded on first use; results are cached by language + code and
 * applied as inline decorations (`mtkN` classes from Monaco's theme stylesheet).
 */

type Monaco = typeof import('monaco-editor');
interface Span {
  from: number;
  to: number;
  cls: string;
}

let monacoP: Promise<{ monaco: Monaco; theme(): string }> | null = null;
const loadMonaco = () =>
  (monacoP ??= import('@/components/editor/monaco').then((m) => ({ monaco: m.monaco as unknown as Monaco, theme: m.currentMonacoTheme })));

const cache = new Map<string, Span[]>();
const CACHE_MAX = 400;

const ALIASES: Record<string, string> = {
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  node: 'javascript',
  ts: 'typescript',
  tsx: 'typescript',
  sh: 'shell',
  bash: 'shell',
  zsh: 'shell',
  console: 'shell',
  ps1: 'powershell',
  pwsh: 'powershell',
  ps: 'powershell',
  py: 'python',
  yml: 'yaml',
  rs: 'rust',
  cs: 'csharp',
  'c#': 'csharp',
  rb: 'ruby',
  kt: 'kotlin',
  md: 'markdown',
  jsonc: 'json',
  json5: 'json',
  gql: 'graphql',
  dockerfile: 'dockerfile',
  docker: 'dockerfile',
  toml: 'ini',
  env: 'ini',
  html: 'html',
  xml: 'xml',
  svg: 'xml',
  'c++': 'cpp',
  h: 'cpp',
  hpp: 'cpp',
  golang: 'go',
  http: 'plaintext',
  text: 'plaintext',
  txt: 'plaintext',
};

/** The Monaco language for a code block's info string (null: don't highlight). */
function resolveLanguage(monaco: Monaco, info: string): string | null {
  const word = info.trim().split(/\s+/)[0]?.toLowerCase().replace(/^\{?\.?/, '').replace(/\}$/, '');
  if (!word) return null;
  const id = ALIASES[word] ?? word;
  if (id === 'plaintext') return null;
  for (const l of monaco.languages.getLanguages()) {
    if (l.id === id || l.aliases?.some((a) => a.toLowerCase() === word) || l.extensions?.includes(`.${word}`)) return l.id;
  }
  return null;
}

/** Colorized spans of `code` (offsets into `code`), or [] when the output can't be lined up. */
async function colorize(code: string, info: string): Promise<Span[]> {
  const { monaco, theme } = await loadMonaco();
  const lang = resolveLanguage(monaco, info);
  if (!lang) return [];
  monaco.editor.setTheme(theme());
  // Tabs would be expanded to spaces: one space each keeps offsets aligned (and tokenizes alike).
  const html = await monaco.editor.colorize(code.replace(/\t/g, ' '), lang, { tabSize: 1 });
  const dom = new DOMParser().parseFromString(`<div>${html}</div>`, 'text/html');
  const spans: Span[] = [];
  let at = 0;
  const walk = (n: Node) => {
    if (n.nodeType === Node.TEXT_NODE) {
      const len = n.textContent?.length ?? 0;
      const cls = (n.parentElement?.className ?? '').split(/\s+/).filter((c) => /^mtk\d+$|^mtk[ibus]$/.test(c) && c !== 'mtk1');
      if (len && cls.length) spans.push({ from: at, to: at + len, cls: cls.join(' ') });
      at += len;
      return;
    }
    if ((n as Element).tagName === 'BR') {
      at += 1;
      return;
    }
    n.childNodes.forEach(walk);
  };
  walk(dom.body.firstChild!);
  return at === code.length || at === code.length + 1 ? spans : [];
}

const key = new PluginKey<DecorationSet>('md-highlight');
const keyOf = (n: PMNode) => `${n.attrs.info}\u0000${n.textContent}`;

/**
 * Updates the decorations of code blocks that changed. `done`: blocks (node identity) whose
 * decorations are in `old` already — unchanged blocks keep theirs (mapped), so typing elsewhere
 * in a long note costs nothing here.
 */
function decorate(doc: PMNode, old: DecorationSet | null, schedule: (n: PMNode) => void, done: WeakSet<PMNode>): DecorationSet {
  let set = old ?? DecorationSet.empty;
  const add: Decoration[] = [];
  const remove: Decoration[] = [];
  doc.descendants((n, pos) => {
    if (n.type !== N.codeBlock) return !n.isTextblock && !n.isAtom;
    if (done.has(n)) return false;
    if (!n.attrs.info) {
      remove.push(...set.find(pos, pos + n.nodeSize));
      done.add(n);
      return false;
    }
    const spans = cache.get(keyOf(n));
    if (!spans) {
      // Keep the (mapped) old colours until the new ones arrive.
      schedule(n);
      return false;
    }
    remove.push(...set.find(pos, pos + n.nodeSize));
    for (const s of spans) add.push(Decoration.inline(pos + 1 + s.from, pos + 1 + s.to, { class: s.cls }));
    done.add(n);
    return false;
  });
  if (remove.length) set = set.remove(remove);
  if (add.length) set = set.add(doc, add);
  return set;
}

export function highlightPlugin() {
  let view: EditorView | null = null;
  const pending = new Set<string>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const queue = new Map<string, PMNode>();
  const flush = () => {
    const jobs = [...queue.values()];
    queue.clear();
    void Promise.all(
      jobs.map((n) =>
        colorize(n.textContent, n.attrs.info as string)
          .catch(() => [] as Span[])
          .then((spans) => {
            const k = keyOf(n);
            pending.delete(k);
            if (cache.size > CACHE_MAX) cache.delete(cache.keys().next().value!);
            cache.set(k, spans);
          }),
      ),
    ).then(() => {
      if (view && !(view as unknown as { isDestroyed?: boolean }).isDestroyed) view.dispatch(view.state.tr.setMeta(key, 'refresh'));
    });
  };
  const schedule = (n: PMNode) => {
    const k = keyOf(n);
    if (pending.has(k) || !n.textContent) return;
    pending.add(k);
    queue.set(k, n);
    clearTimeout(timer);
    timer = setTimeout(flush, 120);
  };
  let done = new WeakSet<PMNode>();
  return new Plugin<DecorationSet>({
    key,
    state: {
      init: (_c, state) => decorate(state.doc, null, schedule, done),
      apply(tr, old) {
        if (tr.getMeta(key) === 'theme') {
          done = new WeakSet();
          return decorate(tr.doc, DecorationSet.empty, schedule, done);
        }
        if (!tr.docChanged && !tr.getMeta(key)) return old;
        return decorate(tr.doc, old.map(tr.mapping, tr.doc), schedule, done);
      },
    },
    props: { decorations: (state) => key.getState(state) },
    view(v) {
      view = v;
      // Monaco's token classes change meaning with the theme: recolour.
      const obs = new MutationObserver(() => {
        cache.clear();
        pending.clear();
        v.dispatch(v.state.tr.setMeta(key, 'theme'));
      });
      obs.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
      return {
        // Also called when the editor loads another document (a new state re-creates plugin views):
        // colouring already queued for it must still run, so the timer stays.
        destroy() {
          obs.disconnect();
          if (view === v) view = null;
        },
      };
    },
  });
}
