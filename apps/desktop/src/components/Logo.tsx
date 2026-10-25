import clsx from 'clsx';
import { usePrefs } from '@/store/prefs';
import { ICON, MARK } from './brandMark';

/**
 * The <D/A> mark on its rounded tile — the app icon, drawn from the same traced path
 * (scripts/brand-assets.py → brandMark.ts). Colours come from `--logo-panel` / `--logo-text`.
 */
export function BrandMark({ size, className }: { size: number; className?: string }) {
  const tile = ICON.size - 2 * ICON.inset;
  return (
    <svg className={className} viewBox={`${ICON.inset} ${ICON.inset} ${tile} ${tile}`} width={size} height={size} aria-hidden>
      <rect x={ICON.inset} y={ICON.inset} width={tile} height={tile} rx={ICON.radius} fill="var(--logo-panel)" />
      <path transform={`translate(${ICON.markX} ${ICON.markY}) scale(${ICON.markScale})`} fill="var(--logo-text)" fillRule="evenodd" d={MARK.path} />
    </svg>
  );
}

/**
 * The long logo: the mark, then "Dev" (bold) "All" (thin). The tile is the accent colour with
 * readable text, or the original grey, per Settings → Appearance.
 */
export function Logo({ height = 22, className }: { height?: number; className?: string }) {
  const style = usePrefs((s) => s.logoStyle);
  return (
    <span className={clsx('logo', style === 'accent' && 'accent', className)} role="img" aria-label="Dev All" style={{ gap: Math.round(height * 0.32) }}>
      <BrandMark size={height} />
      <span className="logo-word" style={{ fontSize: Math.round(height * 0.78) }}>
        <b>Dev</b> <span>All</span>
      </span>
    </span>
  );
}
