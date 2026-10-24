import { useEffect, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { isTauri } from '@/lib/ipc';

/**
 * Who draws the window's title bar (see `build_main_window` in src-tauri/src/lib.rs):
 * - `windows`: no system frame — the top bar is the title bar and draws the caption buttons;
 * - `macos`: overlay title bar — the traffic lights sit on the left of the top bar;
 * - `native`: the system frame (Linux, or the UI in a plain browser).
 */
export type WindowChrome = 'windows' | 'macos' | 'native';

const ua = typeof navigator === 'undefined' ? '' : navigator.userAgent;

export const WINDOW_CHROME: WindowChrome = !isTauri ? 'native' : /Windows/.test(ua) ? 'windows' : /Macintosh|Mac OS X/.test(ua) ? 'macos' : 'native';

export interface WindowState {
  maximized: boolean;
  fullscreen: boolean;
  focused: boolean;
}

/** Maximized / fullscreen / focused state of this window (there are no maximize or fullscreen events: re-read after resizes). */
export function useWindowState(): WindowState {
  const [state, setState] = useState<WindowState>({ maximized: false, fullscreen: false, focused: true });
  useEffect(() => {
    if (WINDOW_CHROME === 'native') return;
    const w = getCurrentWindow();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let alive = true;
    const sync = async () => {
      const [maximized, fullscreen] = await Promise.all([w.isMaximized(), w.isFullscreen()]);
      if (alive) setState((p) => (p.maximized === maximized && p.fullscreen === fullscreen ? p : { ...p, maximized, fullscreen }));
    };
    const subs = [
      // Also fires through the macOS fullscreen animation: debounce.
      w.onResized(() => {
        clearTimeout(timer);
        timer = setTimeout(() => void sync(), 80);
      }),
      w.onFocusChanged(({ payload }) => setState((p) => (p.focused === payload ? p : { ...p, focused: payload }))),
    ];
    void sync();
    return () => {
      alive = false;
      clearTimeout(timer);
      for (const s of subs) void s.then((off) => off());
    };
  }, []);
  return state;
}
