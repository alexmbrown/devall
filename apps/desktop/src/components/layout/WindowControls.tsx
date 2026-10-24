import { getCurrentWindow } from '@tauri-apps/api/window';

// Segoe Fluent Icons / MDL2 glyphs, as in Windows Terminal's caption buttons.
const GLYPH = { minimize: '', maximize: '', restore: '', close: '' };

/**
 * Minimize / maximize / close for the frameless Windows window, at the right end of the top bar
 * (46 px wide, full bar height, red close — like native Windows 11 caption buttons). Not
 * keyboard-focusable, like the native ones: Alt+Space and Alt+F4 still work.
 */
export function WindowControls({ maximized }: { maximized: boolean }) {
  const w = getCurrentWindow();
  return (
    <div className="caption-btns">
      <button className="caption-btn" tabIndex={-1} aria-label="Minimize" title="Minimize" onClick={() => void w.minimize()}>
        {GLYPH.minimize}
      </button>
      <button className="caption-btn" tabIndex={-1} aria-label={maximized ? 'Restore' : 'Maximize'} title={maximized ? 'Restore down' : 'Maximize'} onClick={() => void w.toggleMaximize()}>
        {maximized ? GLYPH.restore : GLYPH.maximize}
      </button>
      <button className="caption-btn close" tabIndex={-1} aria-label="Close" title="Close" onClick={() => void w.close()}>
        {GLYPH.close}
      </button>
    </div>
  );
}
