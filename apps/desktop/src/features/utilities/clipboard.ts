import { useCallback, useEffect, useRef, useState } from 'react';
import { detect, type Detection } from '@/lib/detect';

/** Clipboard text larger than this is never inspected. */
const MAX_CLIPBOARD = 256 * 1024;

/** Values dismissed or already opened this session — the chip doesn't come back for them. */
const handled = new Set<string>();

/**
 * Reads the clipboard when the window regains focus (and once on mount) and reports what it looks
 * like (`lib/detect`). Uses the webview's `navigator.clipboard`; a refused permission or a
 * non-text clipboard is ignored silently. Nothing is sent anywhere.
 */
export function useClipboardDetection(enabled: boolean, current: string) {
  const [found, setFound] = useState<Detection | null>(null);
  const currentRef = useRef(current);
  currentRef.current = current;

  useEffect(() => {
    if (!enabled) {
      setFound(null);
      return;
    }
    let live = true;
    const check = () => {
      if (!document.hasFocus() || !navigator.clipboard?.readText) return;
      navigator.clipboard.readText().then(
        (text) => {
          if (!live) return;
          const d = text && text.length <= MAX_CLIPBOARD ? detect(text) : null;
          setFound(d && !handled.has(d.value) && d.value !== currentRef.current.trim() ? d : null);
        },
        () => {
          /* permission refused or nothing textual: stay quiet */
        },
      );
    };
    check();
    window.addEventListener('focus', check);
    return () => {
      live = false;
      window.removeEventListener('focus', check);
    };
  }, [enabled]);

  const dismiss = useCallback(() => {
    setFound((d) => {
      if (d) handled.add(d.value);
      return null;
    });
  }, []);

  return { found, dismiss };
}
