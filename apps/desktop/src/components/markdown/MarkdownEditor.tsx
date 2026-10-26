import { lazy, Suspense, useEffect, useRef, useState, type ReactNode, type Ref } from 'react';
import clsx from 'clsx';
import { CodeXml, Eye, Link2 } from 'lucide-react';
import { Markdown } from '@/components/Markdown';
import { useTextareaLinks } from '@/components/links/useTextareaLinks';
import { isMac } from '@/components/ui/misc';
import { isLinkShortcut, linkShortcutLabel } from '@/lib/links/insert';

/** Imperative access (Link buttons outside the editor, explicit saves). */
export interface MarkdownEditorHandle {
  focus(): void;
  /** Opens the "Link to…" picker at the caret (a selection becomes the link text). */
  insertLink(): void;
  /** Reports pending edits through `onChange` now. */
  flush(): void;
}

export interface MarkdownEditorProps {
  /** The Markdown. A value other than the last one reported through `onChange` reloads the editor. */
  value: string;
  /** Called with the new Markdown — only when the user changed something (never on load). */
  onChange(md: string): void;
  /** `document`: fills its pane and scrolls (notes); `field`: grows with its content (to-dos, docs, cells). */
  variant?: 'document' | 'field';
  placeholder?: string;
  autoFocus?: boolean | 'end';
  readOnly?: boolean;
  /** Links that aren't URLs or `devall:` (relative paths, `repo:…`). */
  onProjectLink?(href: string): void;
  /** A displayable URL (`blob:`/`data:`) for a relative image path; null when it can't be shown. */
  resolveImage?(src: string): Promise<string | null>;
  /** Extra UI under a code block (the ▶ Run bar of runnable blocks in notes). */
  codeBlockExtras?(block: { language: string; code: string }): ReactNode;
  /** Focus left the editor (not to its own menus or the link picker). */
  onBlur?(): void;
  /** Ctrl/Cmd+Enter (outside task items and code blocks). */
  onSubmit?(): void;
  /** Escape. */
  onEscape?(): void;
  /** Ctrl/Cmd+S (pending edits are reported first). */
  onSave?(): void;
  className?: string;
  ariaLabel?: string;
  handleRef?: Ref<MarkdownEditorHandle>;
}

export type MarkdownEditorImplProps = MarkdownEditorProps;

const load = () => import('./editor/Editor');
const Impl = lazy(load);

/** Starts loading the editor (e.g. on hover) so it's there when needed. */
export const preloadMarkdownEditor = () => void load();

/**
 * WYSIWYG Markdown editing (ProseMirror; Markdown parsed with `marked`, written back preserving the
 * file: untouched blocks stay byte-for-byte as they were, edited ones keep their style). GFM: tables,
 * task lists (clickable), strikethrough, autolinks; code blocks with highlighting; `devall:` links as
 * live chips (`[[` or Ctrl/Cmd+Alt+K to insert); `/` for blocks; Markdown shortcuts while typing.
 *
 * Lazy: the editor is its own chunk; until it's loaded the Markdown is shown read-only.
 */
export function MarkdownEditor(props: MarkdownEditorProps) {
  return (
    <Suspense
      fallback={
        <div className={clsx('md-editor-wrap', props.variant === 'field' ? 'md-field' : 'md-document', props.className)}>
          <Markdown source={props.value} className={clsx('md-editor-fallback', props.variant === 'field' && 'md-field')} />
        </div>
      }
    >
      <Impl {...props} />
    </Suspense>
  );
}

/**
 * A Markdown form field: the editor in a box with a Link button and a Source toggle (plain textarea,
 * `[[` and Ctrl/Cmd+Alt+K still insert links). For to-do descriptions and comments, request docs.
 */
export function MarkdownField({
  value,
  onChange,
  placeholder,
  autoFocus,
  onSubmit,
  onEscape,
  onBlur,
  onProjectLink,
  className,
  ariaLabel,
  bare,
  minHeight,
}: {
  value: string;
  onChange(md: string): void;
  placeholder?: string;
  autoFocus?: boolean | 'end';
  onSubmit?(): void;
  onEscape?(): void;
  onBlur?(): void;
  onProjectLink?(href: string): void;
  className?: string;
  ariaLabel?: string;
  /** No border until hovered/focused (inline in a panel). */
  bare?: boolean;
  minHeight?: number;
}) {
  const [source, setSource] = useState(false);
  const ed = useRef<MarkdownEditorHandle>(null);
  const box = useRef<HTMLDivElement>(null);
  const linker = useTextareaLinks(onChange);
  // Switching to the source: the caret at the end, where typing usually continues.
  useEffect(() => {
    const el = linker.ref.current;
    if (source && el) el.setSelectionRange(el.value.length, el.value.length);
  }, [source, linker.ref]);
  return (
    <div
      ref={box}
      className={clsx('md-fieldbox', bare && 'bare', className)}
      style={minHeight ? { minHeight } : undefined}
      onBlur={(e) => {
        if (!box.current?.contains(e.relatedTarget as Node | null) && !linker.picking.current && source) onBlur?.();
      }}
    >
      {source ? (
        <textarea
          ref={linker.ref}
          className="textarea md-field-source"
          autoFocus
          aria-label={ariaLabel ? `${ariaLabel} (Markdown source)` : 'Markdown source'}
          rows={Math.min(18, Math.max(3, value.split('\n').length + 1))}
          style={minHeight ? { minHeight: minHeight - 2 } : undefined}
          value={value}
          placeholder={placeholder}
          onChange={linker.onChange}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (linker.onKeyDown(e)) return;
            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && onSubmit) {
              e.preventDefault();
              onSubmit();
            } else if (e.key === 'Escape' && onEscape) onEscape();
            else if (isLinkShortcut(e, isMac)) e.preventDefault();
          }}
        />
      ) : (
        <MarkdownEditor
          variant="field"
          handleRef={ed}
          value={value}
          onChange={onChange}
          placeholder={placeholder}
          autoFocus={autoFocus}
          onSubmit={onSubmit}
          onEscape={onEscape}
          onBlur={onBlur}
          onProjectLink={onProjectLink}
          ariaLabel={ariaLabel}
        />
      )}
      <div className="md-field-tools">
        <button
          className="icon-btn sm"
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => (source ? void linker.insert() : ed.current?.insertLink())}
          title={`Link to a request, note, to-do… (${linkShortcutLabel(isMac)}, or type [[)`}
          aria-label="Insert link"
        >
          <Link2 />
        </button>
        <button
          className={clsx('icon-btn sm', source && 'on')}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            ed.current?.flush();
            setSource(!source);
          }}
          title={source ? 'Back to rich text' : 'Edit the Markdown source'}
          aria-label={source ? 'Rich text' : 'Markdown source'}
          aria-pressed={source}
        >
          {source ? <Eye /> : <CodeXml />}
        </button>
      </div>
    </div>
  );
}
