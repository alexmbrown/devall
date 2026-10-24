import { useState } from 'react';

/**
 * The /dev/all wordmark: a light panel with dark "dev" and "all" and a transparent "/" cut-out,
 * drawn for dark backgrounds.
 *
 * Uses the original artwork at `public/brand/logo.png` — as-is in the dark theme, colour-inverted
 * in the light theme (`.logo-img` in devall.css), so the cut-out slash always shows the page behind
 * it. If the PNG is missing, a vector copy with the same cut-out is drawn from `--logo-*` tokens.
 */
export function Logo({ height = 22, className }: { height?: number; className?: string }) {
  const [missing, setMissing] = useState(false);
  if (!missing)
    return <img className={`logo-img ${className ?? ''}`} src="./brand/logo.png" alt="/dev/all" style={{ height }} onError={() => setMissing(true)} draggable={false} />;
  return (
    <svg className={className} viewBox="0 0 171 57" height={height} width={(height * 171) / 57} role="img" aria-label="/dev/all">
      <defs>
        <mask id="devall-logo-slash">
          <rect width="171" height="57" fill="#fff" />
          <polygon points="88,57 100,57 120,0 108,0" fill="#000" />
        </mask>
      </defs>
      <rect width="171" height="57" fill="var(--logo-panel)" mask="url(#devall-logo-slash)" />
      <text x="6" y="41" fontSize="40" fill="var(--logo-text)" fontFamily="'Segoe UI', 'Helvetica Neue', Arial, system-ui, sans-serif">
        dev
      </text>
      <text x="118" y="41" fontSize="40" fill="var(--logo-text)" fontFamily="'Segoe UI', 'Helvetica Neue', Arial, system-ui, sans-serif">
        all
      </text>
    </svg>
  );
}
