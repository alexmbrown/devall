import { useCallback, useRef } from 'react';
import { isMac } from '@/components/ui/misc';
import { isLinkShortcut, replaceRange, wikiTrigger } from '@/lib/links/insert';
import { markdownLink } from '@/lib/links/syntax';
import { pickLink } from './LinkPicker';

/** Roughly where the caret's line is in a textarea (viewport px), for anchoring the picker. */
function caretPoint(el: HTMLTextAreaElement): { x: number; y: number } {
  const r = el.getBoundingClientRect();
  const cs = getComputedStyle(el);
  const lineHeight = parseFloat(cs.lineHeight) || 18;
  const line = el.value.slice(0, el.selectionStart).split('\n').length;
  const y = r.top + (parseFloat(cs.paddingTop) || 0) + line * lineHeight - el.scrollTop;
  return { x: r.left + (parseFloat(cs.paddingLeft) || 0), y: Math.min(r.bottom, Math.max(r.top, y)) };
}

/**
 * Link insertion for a Markdown textarea: typing `[[`, the Link button or Ctrl+Alt+K / ⌘⌥K opens
 * the "Link to…" picker and inserts `[Title](devall:…)` (a selection becomes the link text).
 * `picking` is true while the picker has focus — treat the textarea's blur as not-leaving then.
 */
export function useTextareaLinks(setValue: (v: string) => void) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const picking = useRef(false);

  const insert = useCallback(
    async (range?: { from: number; to: number }) => {
      const el = ref.current;
      if (!el || picking.current) return;
      const from = range?.from ?? el.selectionStart;
      const to = range?.to ?? el.selectionEnd;
      const selected = range ? '' : el.value.slice(from, to).trim();
      picking.current = true;
      const picked = await pickLink({ anchor: caretPoint(el) });
      picking.current = false;
      const now = ref.current;
      if (!now) return;
      now.focus();
      if (!picked) return;
      const r = replaceRange(now.value, from, to, markdownLink(selected || picked.title, picked.link));
      setValue(r.text);
      requestAnimationFrame(() => ref.current?.setSelectionRange(r.caret, r.caret));
    },
    [setValue],
  );

  return {
    ref,
    picking,
    insert,
    /** Use as the textarea's onChange: keeps the value and opens the picker on `[[`. */
    onChange(e: React.ChangeEvent<HTMLTextAreaElement>) {
      const v = e.target.value;
      setValue(v);
      const typed = (e.nativeEvent as InputEvent).inputType === 'insertText';
      const t = typed ? wikiTrigger(v, e.target.selectionStart) : null;
      if (t) void insert(t);
    },
    /** Call first in the textarea's onKeyDown; true when it handled the key. */
    onKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>): boolean {
      if (!isLinkShortcut(e, isMac)) return false;
      e.preventDefault();
      void insert();
      return true;
    },
  };
}
