import { useEffect } from 'react';
import { useNav, type Section } from '@/store/nav';
import { usePalette } from '@/features/palette/CommandPalette';
import { useWorkbench, activeViewOf } from '@/store/workbench';
import { useWorkspace } from '@/store/workspace';

function isTyping(e: KeyboardEvent) {
  // Keys dispatched on document/window have a non-element target.
  const t = e.target;
  if (!(t instanceof HTMLElement)) return false;
  return t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName) || !!t.closest('.monaco-editor');
}

/** A modal dialog or the palette owns the keyboard. */
const dialogOpen = () => !!document.querySelector('.overlay');

const GO_KEYS: Record<string, Section> = { r: 'http', e: 'environments', u: 'utilities', p: 'projects', s: 'settings' };

/**
 * Workbench keys (work while typing, like an editor): handled in the capture phase.
 *  Ctrl+Tab / Ctrl+Shift+Tab   next / previous tab of the active group (Ctrl on macOS too)
 *  Ctrl/Cmd+W                  close the active tab (pinned tabs stay)
 *  Ctrl/Cmd+\                  split right (duplicates the active view where possible)
 *  Ctrl/Cmd+1…4                focus editor group 1…4
 */
function onWorkbenchKey(e: KeyboardEvent): boolean {
  if (dialogOpen()) return false;
  const wb = useWorkbench.getState();
  if (e.ctrlKey && !e.altKey && e.key === 'Tab') {
    wb.cycle(e.shiftKey ? -1 : 1);
    return true;
  }
  const mod = e.ctrlKey || e.metaKey;
  if (!mod || e.altKey || e.shiftKey) return false;
  const key = e.key.toLowerCase();
  if (key === 'w') {
    const v = activeViewOf(wb);
    if (v && !v.pinned) void wb.close(v.id);
    return true;
  }
  if (e.key === '\\' || e.code === 'Backslash') {
    wb.split(activeViewOf(wb)?.id ?? null, 'right');
    return true;
  }
  if (/^Digit[1-4]$/.test(e.code)) {
    wb.focusGroupIndex(Number(e.code.slice(5)) - 1);
    return true;
  }
  return false;
}

/**
 * Global keys:
 *  Ctrl/Cmd+K  command palette
 *  Ctrl/Cmd+N  new request
 *  g then <key>  jump to a module (g r = requests, g e = environments, g u = utilities)
 *  + the workbench keys above
 */
export function useGlobalShortcuts() {
  useEffect(() => {
    let gPressed = 0;
    const onCapture = (e: KeyboardEvent) => {
      if (onWorkbenchKey(e)) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      const key = e.key.toLowerCase();
      // Ctrl/Cmd+Alt+K is "Link to…" in Markdown editors.
      if (mod && !e.altKey && key === 'k') {
        e.preventDefault();
        usePalette.getState().setOpen(!usePalette.getState().open);
        return;
      }
      // Ctrl/Cmd+O: switch project — the palette filtered to recent projects (+ open/new folder).
      if (mod && !e.altKey && !e.shiftKey && key === 'o') {
        e.preventDefault();
        usePalette.getState().setOpen(true, 'recent ');
        return;
      }
      if (mod && key === 'n' && useWorkspace.getState().project) {
        e.preventDefault();
        useWorkspace.getState().newRequest('');
        return;
      }
      if (isTyping(e) || mod || e.altKey) return;
      if (key === 'g') {
        gPressed = Date.now();
        return;
      }
      if (gPressed && Date.now() - gPressed < 1200) {
        const sec = GO_KEYS[key];
        gPressed = 0;
        if (sec) {
          e.preventDefault();
          useNav.getState().go(sec);
        }
      }
    };
    window.addEventListener('keydown', onCapture, true);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('keydown', onCapture, true);
      window.removeEventListener('keydown', onKey);
    };
  }, []);
}
