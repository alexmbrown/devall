import { memo, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';
import clsx from 'clsx';
import { AppWindow, Copy, ExternalLink, Link2, Pencil, PanelRight, Trash2, Unlink } from 'lucide-react';
import { Fragment, Slice, type Node as PMNode } from 'prosemirror-model';
import { EditorState, NodeSelection, Selection, TextSelection } from 'prosemirror-state';
import { dropCursor } from 'prosemirror-dropcursor';
import { gapCursor } from 'prosemirror-gapcursor';
import { history } from 'prosemirror-history';
import { isInTable, tableEditing } from 'prosemirror-tables';
import { EditorView } from 'prosemirror-view';
import { openUrl } from '@tauri-apps/plugin-opener';
import { copyLinkItems, useFollowLink } from '@/components/links/LinkChip';
import { pickLink as openLinkPicker } from '@/components/links/LinkPicker';
import { MenuPopup, type MenuItemDef } from '@/components/ui/Menu';
import { copyText, isMac } from '@/components/ui/misc';
import { promptDialog } from '@/components/ui/dialogs';
import { isDevallLink } from '@/lib/links/syntax';
import type { MarkdownEditorImplProps } from '../MarkdownEditor';
import { insertDevallLink, linkAt, retargetDevallLink, setLink, toggleTask } from '../commands';
import { parseFragment, parseMarkdown, type MdSource } from '../parse';
import { N, schema } from '../schema';
import { serializeBlocks, serializeMarkdown } from '../serialize';
import { highlightPlugin } from './highlight';
import { filterSlash, LinkBubble, runSlash, SelectionToolbar, SlashMenu, slashItems, TableBar, type SlashItem } from './menus';
import { nodeViews, type ViewHost } from './nodeViews';
import { closeSlash, markdownInputRules, markdownKeymap, placeholderPlugin, slashKey, slashPlugin, wikiLinkTrigger, type PluginHost } from './plugins';
import { Portals } from './portals';
import '../markdown-editor.css';

/**
 * The React content of node views (chips, code block bars). Its own component so the editor's
 * re-render on every transaction (menus) doesn't re-render hundreds of portals.
 */
const PortalList = memo(function PortalList({ portals }: { portals: Portals }) {
  useSyncExternalStore(portals.subscribe, portals.snapshot);
  return <>{portals.entries().map(([el, { key, node }]) => createPortal(node, el, `p${key}`))}</>;
});

/** Text that is most likely Markdown source (pasted from an editor, a README, a chat). */
const looksLikeMarkdown = (t: string) => /(^|\n) {0,3}(#{1,6} |[-*+] |\d{1,9}[.)] |> |```|~~~|\|.+\|)|\*\*[^*\n]+\*\*|__[^_\n]+__|\[[^\]\n]+\]\([^)\n]+\)|`[^`\n]+`|(^|\n)- \[[ xX]\] /.test(t);
const URL_RE = /^(https?:\/\/|mailto:)\S+$/i;

/**
 * The WYSIWYG Markdown editor (ProseMirror). Loaded lazily through `MarkdownEditor`. See
 * `MarkdownEditor.tsx` for the props and DEVELOPING.md ("Markdown editing") for the rules.
 */
export default function Editor(props: MarkdownEditorImplProps) {
  const { variant = 'document', className, ariaLabel } = props;
  const hostRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const sourceRef = useRef<MdSource | null>(null);
  const emitted = useRef(props.value);
  const propsRef = useRef(props);
  propsRef.current = props;
  const portals = useMemo(() => new Portals(), []);
  const [, setTick] = useState(0);
  const [focused, setFocused] = useState(false);
  const [menu, setMenu] = useState<{ point: { x: number; y: number }; items: MenuItemDef[] } | null>(null);
  const [slashHl, setSlashHl] = useState(0);
  const follow = useFollowLink();
  const followRef = useRef(follow);
  followRef.current = follow;
  const picking = useRef(false);

  /** Writes the document back to Markdown and reports it — only when it differs from the last value. */
  const emit = () => {
    const view = viewRef.current;
    if (!view) return;
    const md = serializeMarkdown(view.state.doc, sourceRef.current);
    if (md === emitted.current) return;
    emitted.current = md;
    propsRef.current.onChange(md);
  };

  const openHref = (href: string, e?: { ctrlKey: boolean; metaKey: boolean; button?: number }) => {
    if (isDevallLink(href)) followRef.current(href, e);
    else if (/^(https?:|mailto:)/i.test(href)) void openUrl(href).catch(() => {});
    else if (href) propsRef.current.onProjectLink?.(decodeURIComponent(href.replace(/^\.?\//, '')));
  };

  /** The "Link to…" picker at the caret; `range` (the typed `[`) is replaced by the pick. */
  const pickLink = async (range?: { from: number; to: number }) => {
    const view = viewRef.current;
    if (!view || picking.current || propsRef.current.readOnly) return;
    const at = Math.min(range?.from ?? view.state.selection.from, view.state.doc.content.size);
    const c = view.coordsAtPos(at);
    picking.current = true;
    const picked = await openLinkPicker({ anchor: { x: c.left, y: c.bottom } });
    picking.current = false;
    const v = viewRef.current;
    if (!v) return;
    v.focus();
    const valid = range && range.to <= v.state.doc.content.size && v.state.doc.textBetween(range.from, range.from + 1) === '[';
    if (!picked) {
      // `[[` typed but nothing picked: keep what was typed.
      if (valid) v.dispatch(v.state.tr.insertText('[', range!.to));
      return;
    }
    v.dispatch(insertDevallLink(v.state, picked, valid ? range : undefined));
  };

  const editWebLink = async () => {
    const view = viewRef.current;
    if (!view) return;
    const cur = linkAt(view.state);
    const href = await promptDialog({ title: cur ? 'Edit link' : 'Add link', label: 'URL (empty removes the link)', initial: cur?.href ?? '', placeholder: 'https://…' });
    const v = viewRef.current;
    if (!v || href === null) return;
    v.focus();
    v.dispatch(setLink(v.state, href));
  };

  // Slash menu items and keyboard (read through refs by the plugin).
  const slash = useMemo(() => slashItems({ pickLink: () => void pickLink(), webLink: () => void editWebLink() }), []);
  const slashState = viewRef.current ? slashKey.getState(viewRef.current.state) : null;
  const shown = slashState ? filterSlash(slash, slashState.query) : [];
  const slashRef = useRef({ shown, hl: slashHl });
  slashRef.current = { shown, hl: Math.min(slashHl, Math.max(0, shown.length - 1)) };
  useEffect(() => setSlashHl(0), [slashState?.query, slashState?.from]);

  useLayoutEffect(() => {
    const { doc, source } = parseMarkdown(props.value);
    sourceRef.current = source;
    emitted.current = props.value;
    const pluginHost: PluginHost = {
      pickLink: (range) => void pickLink(range),
      onSubmit: () => (propsRef.current.onSubmit ? (propsRef.current.onSubmit(), true) : false),
      onEscape: () => {
        if (!propsRef.current.onEscape) return false;
        emit();
        propsRef.current.onEscape();
        return true;
      },
      onSave: () => {
        // Pending edits first, so whoever saves (this prop or a page-level Ctrl+S) has them.
        emit();
        if (!propsRef.current.onSave) return false;
        propsRef.current.onSave();
        return true;
      },
      slashKey: (key) => {
        const view = viewRef.current;
        if (!view || !slashKey.getState(view.state)) return false;
        const { shown: items, hl } = slashRef.current;
        if (key === 'Escape') {
          closeSlash(view);
          return true;
        }
        if (!items.length) return false;
        if (key === 'ArrowDown') setSlashHl((hl + 1) % items.length);
        else if (key === 'ArrowUp') setSlashHl((hl - 1 + items.length) % items.length);
        else runSlash(view, items[hl]);
        return true;
      },
      placeholder: () => propsRef.current.placeholder,
    };
    const viewHost: ViewHost = {
      portals,
      readOnly: () => !!propsRef.current.readOnly,
      resolveImage: (src) => propsRef.current.resolveImage?.(src) ?? Promise.resolve(null),
      codeBlockExtras: (block) => propsRef.current.codeBlockExtras?.(block),
      focus: () => viewRef.current?.focus(),
    };
    const plugins = [
      slashPlugin(),
      wikiLinkTrigger(pluginHost),
      markdownInputRules(),
      ...markdownKeymap(pluginHost),
      history(),
      dropCursor({ class: 'md-dropcursor' }),
      gapCursor(),
      tableEditing(),
      highlightPlugin(),
      placeholderPlugin(pluginHost),
    ];
    const view: EditorView = new EditorView(hostRef.current!, {
      state: EditorState.create({ doc, plugins }),
      nodeViews: nodeViews(viewHost),
      editable: () => !propsRef.current.readOnly,
      attributes: () => ({
        class: clsx('markdown md-editor', variant === 'field' && 'md-field'),
        role: 'textbox',
        'aria-multiline': 'true',
        'aria-label': propsRef.current.ariaLabel ?? 'Markdown',
        spellcheck: 'true',
      }),
      dispatchTransaction: (tr) => {
        view.updateState(view.state.apply(tr));
        if (tr.docChanged) emit();
        setTick((t) => t + 1);
      },
      handleDOMEvents: {
        mousedown(v, e) {
          const t = e.target as HTMLElement;
          const box = t.closest('.md-task-box');
          if (box) {
            e.preventDefault();
            if (propsRef.current.readOnly) return true;
            const body = box.parentElement?.querySelector(':scope > .md-task-body');
            if (!body) return true;
            const tr = toggleTask(v.state, v.posAtDOM(body, 0));
            if (tr) v.dispatch(tr);
            return true;
          }
          if (e.button === 1 && t.closest('.md-chip-host, a.md-link')) e.preventDefault();
          return false;
        },
        click(v, e) {
          const t = e.target as HTMLElement;
          const chip = t.closest('.md-chip-host') as HTMLElement | null;
          if (chip?.dataset.href) {
            e.preventDefault();
            openHref(chip.dataset.href, e);
            return true;
          }
          const a = t.closest('a.md-link') as HTMLAnchorElement | null;
          if (a) {
            e.preventDefault();
            // Ctrl/Cmd+click follows; a plain click puts the caret there (the link bubble offers Open).
            if (e.ctrlKey || e.metaKey || propsRef.current.readOnly) openHref(a.getAttribute('href') ?? '', { ctrlKey: false, metaKey: false });
            return true;
          }
          void v;
          return false;
        },
        auxclick(_v, e) {
          if (e.button !== 1) return false;
          const chip = (e.target as HTMLElement).closest('.md-chip-host') as HTMLElement | null;
          if (!chip?.dataset.href) return false;
          e.preventDefault();
          openHref(chip.dataset.href, { ctrlKey: false, metaKey: false, button: 1 });
          return true;
        },
        contextmenu(v, e) {
          const t = e.target as HTMLElement;
          const chip = t.closest('.md-chip-host') as HTMLElement | null;
          if (chip?.dataset.href) {
            e.preventDefault();
            let pos = v.posAtDOM(chip, 0);
            if (v.state.doc.nodeAt(pos)?.type !== N.devallLink && pos > 0) pos -= 1;
            const node = v.state.doc.nodeAt(pos);
            if (node?.type !== N.devallLink) return true;
            v.dispatch(v.state.tr.setSelection(NodeSelection.create(v.state.doc, pos)));
            setMenu({ point: { x: e.clientX, y: e.clientY }, items: chipMenu(v, pos, node) });
            return true;
          }
          const a = t.closest('a.md-link') as HTMLAnchorElement | null;
          if (a) {
            e.preventDefault();
            const href = a.getAttribute('href') ?? '';
            setMenu({
              point: { x: e.clientX, y: e.clientY },
              items: [
                { label: 'Open link', icon: <ExternalLink />, onSelect: () => openHref(href) },
                { label: 'Copy link', icon: <Copy />, onSelect: () => void copyText(href, 'Link copied') },
                ...(propsRef.current.readOnly
                  ? []
                  : [
                      { label: 'Edit link…', icon: <Pencil />, onSelect: () => void editWebLink() },
                      {
                        label: 'Remove link',
                        icon: <Unlink />,
                        onSelect: () => {
                          const view = viewRef.current;
                          const l = view && linkAt(view.state);
                          if (view && l) view.dispatch(view.state.tr.removeMark(l.from, l.to, schema.marks.link));
                        },
                      },
                    ]),
              ],
            });
            return true;
          }
          return false;
        },
        focus() {
          setFocused(true);
          return false;
        },
        blur() {
          setFocused(false);
          emit();
          return false;
        },
      },
      handleKeyDown(v, e) {
        // Enter on a selected chip opens it (Ctrl/Cmd+Enter: to the side).
        const sel = v.state.selection;
        if (e.key === 'Enter' && sel instanceof NodeSelection && sel.node.type === N.devallLink) {
          openHref(sel.node.attrs.href, e);
          return true;
        }
        return false;
      },
      clipboardTextSerializer(slice, v) {
        if (v.state.selection.$from.parent.type.spec.code) return slice.content.textBetween(0, slice.content.size, '\n');
        const nodes: PMNode[] = [];
        slice.content.forEach((n) => nodes.push(n));
        if (nodes.length && nodes[0].isInline) return serializeBlocks([N.paragraph.create(null, slice.content)], sourceRef.current);
        return serializeBlocks(
          nodes.map((n) => (n.type === N.listItem ? N.bulletList.create(null, n) : n.type === N.tableRow ? N.table.create(null, n) : n)),
          sourceRef.current,
        );
      },
      clipboardTextParser(text, $context) {
        if ($context.parent.type.spec.code) return new Slice(Fragment.from(text ? schema.text(text) : []), 0, 0);
        return Slice.maxOpen(parseFragment(text, sourceRef.current?.links).content);
      },
      handlePaste(v, e) {
        const data = e.clipboardData;
        if (!data || v.state.selection.$from.parent.type.spec.code) return false;
        const text = data.getData('text/plain');
        const html = data.getData('text/html');
        if (!text || data.files.length) return false;
        // A URL pasted over selected text links it.
        if (URL_RE.test(text.trim()) && !v.state.selection.empty) {
          v.dispatch(setLink(v.state, text.trim()));
          return true;
        }
        if (html && /data-pm-slice/.test(html)) return false;
        const fromCode = /vscode|monaco|<pre|white-space:\s*pre/i.test(html);
        if (html && !fromCode && !looksLikeMarkdown(text)) return false;
        const slice = Slice.maxOpen(parseFragment(text, sourceRef.current?.links).content);
        v.dispatch(v.state.tr.replaceSelection(slice).scrollIntoView());
        return true;
      },
    });
    viewRef.current = view;
    if (propsRef.current.autoFocus) {
      view.focus();
      if (propsRef.current.autoFocus === 'end') view.dispatch(view.state.tr.setSelection(Selection.atEnd(view.state.doc)));
    }
    setTick((t) => t + 1);
    return () => {
      emit();
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- created once; props are read through refs
  }, []);

  /** Right-click on a chip. */
  const chipMenu = (v: EditorView, pos: number, node: PMNode): MenuItemDef[] => {
    const href = node.attrs.href as string;
    const ro = !!propsRef.current.readOnly;
    return [
      { label: 'Open', icon: <AppWindow />, onSelect: () => openHref(href) },
      { label: 'Open to the side', icon: <PanelRight />, hint: isMac ? '⌘-click' : 'Ctrl+click', onSelect: () => openHref(href, { ctrlKey: true, metaKey: false }) },
      ...copyLinkItems(href, node.attrs.label as string),
      ...(ro
        ? []
        : [
            { separator: true },
            {
              label: 'Change link…',
              icon: <Link2 />,
              onSelect: async () => {
                const c = v.coordsAtPos(pos);
                const picked = await openLinkPicker({ anchor: { x: c.left, y: c.bottom } });
                const view = viewRef.current;
                const cur = view?.state.doc.nodeAt(pos);
                if (!view || !picked || cur?.type !== N.devallLink) return;
                view.dispatch(retargetDevallLink(view.state, pos, picked, false));
                view.focus();
              },
            },
            {
              label: 'Edit text…',
              icon: <Pencil />,
              onSelect: async () => {
                const label = await promptDialog({ title: 'Link text', label: 'Shown when the item can’t be found (Markdown)', initial: node.attrs.label as string });
                const view = viewRef.current;
                const cur = view?.state.doc.nodeAt(pos);
                if (!view || label === null || cur?.type !== N.devallLink) return;
                view.dispatch(view.state.tr.setNodeMarkup(pos, undefined, { ...cur.attrs, label, raw: null }));
                view.focus();
              },
            },
            {
              label: 'Remove link (keep text)',
              icon: <Unlink />,
              onSelect: () => {
                const view = viewRef.current;
                const cur = view?.state.doc.nodeAt(pos);
                if (!view || cur?.type !== N.devallLink) return;
                const text = (cur.attrs.label as string).replace(/\\(.)/g, '$1');
                view.dispatch(view.state.tr.replaceWith(pos, pos + cur.nodeSize, text ? schema.text(text, cur.marks) : []));
                view.focus();
              },
            },
            {
              label: 'Delete',
              icon: <Trash2 />,
              danger: true,
              onSelect: () => {
                const view = viewRef.current;
                if (!view || view.state.doc.nodeAt(pos)?.type !== N.devallLink) return;
                view.dispatch(view.state.tr.delete(pos, pos + 1));
                view.focus();
              },
            },
          ]),
    ];
  };

  // A new value from outside (disk reload, the Source view): load it.
  useEffect(() => {
    const view = viewRef.current;
    if (!view || props.value === emitted.current) return;
    const { doc, source } = parseMarkdown(props.value);
    sourceRef.current = source;
    emitted.current = props.value;
    const pos = Math.min(view.state.selection.from, doc.content.size);
    const state = EditorState.create({ doc, plugins: view.state.plugins });
    view.updateState(state.apply(state.tr.setSelection(Selection.near(doc.resolve(pos)))));
    setTick((t) => t + 1);
  }, [props.value]);

  useEffect(() => {
    viewRef.current?.setProps({});
  }, [props.readOnly]);

  useImperativeHandle(
    props.handleRef,
    () => ({
      focus: () => viewRef.current?.focus(),
      insertLink: () => void pickLink(),
      flush: emit,
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  const view = viewRef.current;
  const sel = view?.state.selection;
  const editable = !props.readOnly;
  const showToolbar = !!view && focused && editable && !!sel && !sel.empty && sel instanceof TextSelection && !sel.$from.parent.type.spec.code && !slashState;
  const showLink = !!view && focused && !!sel && sel.empty && !slashState && !!linkAt(view.state);
  const showTable = !!view && focused && editable && !showToolbar && isInTable(view.state);

  return (
    <div
      ref={wrapRef}
      className={clsx('md-editor-wrap', `md-${variant}`, className)}
      onBlur={(e) => {
        if (!wrapRef.current?.contains(e.relatedTarget as Node | null) && !picking.current) propsRef.current.onBlur?.();
      }}
      aria-label={ariaLabel}
    >
      <div ref={hostRef} className="md-editor-host" />
      <PortalList portals={portals} />
      {view && showToolbar && <SelectionToolbar view={view} onLink={() => void editWebLink()} onPick={() => void pickLink()} />}
      {view && showLink && <LinkBubble view={view} onOpen={(href) => openHref(href)} onEdit={() => void editWebLink()} />}
      {view && showTable && <TableBar view={view} />}
      {view && slashState && focused && (
        <SlashMenu
          view={view}
          items={shown}
          hl={slashRef.current.hl}
          onHl={setSlashHl}
          onPick={(it: SlashItem) => {
            runSlash(view, it);
          }}
        />
      )}
      {menu && <MenuPopup point={menu.point} items={menu.items} onClose={() => setMenu(null)} />}
    </div>
  );
}
