/** Tiny line chart for live stats (last N samples, fixed or auto max). Colors come from tokens. */
export function Sparkline({ values, max, width = 160, height = 36, color = 'var(--accent)', label }: { values: number[]; max?: number; width?: number; height?: number; color?: string; label: string }) {
  const top = Math.max(max ?? 0, ...values, 1e-9);
  const n = values.length;
  const pts = values.map((v, i) => `${n <= 1 ? width : (i / (n - 1)) * width},${height - 2 - (Math.max(0, v) / top) * (height - 4)}`).join(' ');
  return (
    <svg className="sparkline" width={width} height={height} viewBox={`0 0 ${width} ${height}`} role="img" aria-label={label}>
      <line x1={0} x2={width} y1={height - 1} y2={height - 1} stroke="var(--chart-grid)" strokeWidth={1} />
      {n > 0 && (
        <>
          <polygon points={`0,${height - 1} ${pts} ${width},${height - 1}`} fill={color} opacity={0.12} />
          <polyline points={pts} fill="none" stroke={color} strokeWidth={1.5} strokeLinejoin="round" strokeLinecap="round" />
        </>
      )}
    </svg>
  );
}
