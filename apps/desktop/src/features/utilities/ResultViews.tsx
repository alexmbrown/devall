import { Fragment, useMemo, type ReactNode } from 'react';
import clsx from 'clsx';
import { AlertTriangle, CheckCircle2, ListOrdered, XCircle } from 'lucide-react';
import { Callout, CopyButton } from '@/components/ui/misc';
import type { ColorResult, CronResult, MagicResult } from '@/lib/api/utils';
import { useUtilities } from './shared';

function parse<T>(output: string): T | null {
  try {
    return JSON.parse(output) as T;
  } catch {
    return null;
  }
}

/** Rich renderers for some ops' output; `null` = fall back to the raw editor. */
export function richView(opId: string, output: string, input: string): ReactNode | null {
  if (!output) return null;
  switch (opId) {
    case 'qr.generate':
      return <QrView svg={output} />;
    case 'cron.explain': {
      const r = parse<CronResult>(output);
      return r && <CronView r={r} />;
    }
    case 'detect.magic': {
      const r = parse<MagicResult>(output);
      return r && <MagicView r={r} input={input} />;
    }
    case 'color.convert': {
      const r = parse<ColorResult>(output);
      return r && <ColorView r={r} />;
    }
    default:
      return null;
  }
}

export const RICH_VIEWS = new Set(['qr.generate', 'cron.explain', 'detect.magic', 'color.convert']);

function QrView({ svg }: { svg: string }) {
  // Rendered through <img>, so nothing inside the SVG can execute.
  const src = useMemo(() => `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`, [svg]);
  return (
    <div className="util-rich col">
      <div className="util-qr">
        <img src={src} alt="QR code" />
      </div>
      <div className="row">
        <CopyButton text={svg} label="Copy SVG" />
        <CopyButton text={src} label="Copy data URL" />
      </div>
    </div>
  );
}

function CronView({ r }: { r: CronResult }) {
  return (
    <div className="util-rich col">
      <div className="util-cron-desc">{r.description}</div>
      <div className="row wrap">
        {Object.entries(r.fields).map(([k, v]) => (
          <span key={k} className="tag" title={k}>
            <span className="subtle">{k}</span> <code className="mono">{v}</code>
          </span>
        ))}
      </div>
      {r.warnings.map((w) => (
        <Callout key={w} kind="warning">
          {w}
        </Callout>
      ))}
      <div className="section-title row-sm">
        <ListOrdered size={14} /> Next runs <span className="subtle xs">({r.timezone})</span>
      </div>
      <table className="table util-nowrap">
        <thead>
          <tr>
            <th>#</th>
            <th>Time</th>
            <th>Day</th>
            <th>In</th>
            {r.timezone !== 'UTC' && <th>UTC</th>}
          </tr>
        </thead>
        <tbody>
          {r.nextRuns.map((n, i) => (
            <tr key={n.utc}>
              <td className="subtle">{i + 1}</td>
              <td className="mono">{n.display}</td>
              <td>{n.weekday}</td>
              <td className="muted">{n.relative}</td>
              {r.timezone !== 'UTC' && <td className="mono subtle">{n.utc.replace('T', ' ')}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function MagicView({ r, input }: { r: MagicResult; input: string }) {
  const { open } = useUtilities();
  if (!r.candidates.length)
    return (
      <div className="util-rich">
        <Callout>Nothing decodable found ({r.explored} attempts). The input looks like plain text.</Callout>
      </div>
    );
  const top = r.candidates[0].score;
  return (
    <div className="util-rich col">
      {r.candidates.map((c, i) => (
        <div key={i} className="card util-magic">
          <div className="row">
            <strong className="grow">{c.label}</strong>
            <span className="util-score" title={c.reasons.join(' · ')}>
              <span style={{ width: `${Math.max(4, Math.min(100, (c.score / Math.max(top, 1)) * 100))}%` }} />
            </span>
            <span className="subtle xs mono">{c.score.toFixed(0)}</span>
            <button className="btn btn-sm" onClick={() => open('recipe.run', { input, args: { steps: JSON.stringify(c.recipe) } })} title="Open this chain in the recipe builder">
              Open as recipe
            </button>
          </div>
          <div className="subtle xs">{c.reasons.join(' · ')}</div>
          <pre className="util-preview">
            {c.preview}
            {c.truncated && '…'}
          </pre>
        </div>
      ))}
    </div>
  );
}

function Pass({ ok, label }: { ok: boolean; label: string }) {
  return (
    <span className={clsx('badge', ok ? 'success' : 'danger')}>
      {ok ? <CheckCircle2 /> : <XCircle />} {label}
    </span>
  );
}

function ColorView({ r }: { r: ColorResult }) {
  const formats: [string, string | null][] = [
    ['HEX', r.hexAlpha ?? r.hex],
    ['RGB', r.rgb],
    ['HSL', r.hsl],
    ['HSV', r.hsv],
    ['OKLCH', r.oklch],
    ['OKLab', r.oklab],
  ];
  const color = r.hexAlpha ?? r.hex;
  return (
    <div className="util-rich col">
      <div className="util-swatch" style={{ background: color, color: r.bestTextColor }}>
        <span className="mono">{color}</span>
        {r.nearestNamed && <span className="xs">{r.nearestNamed.exact ? r.nearestNamed.name : `≈ ${r.nearestNamed.name}`}</span>}
      </div>
      {r.outOfSrgbGamut && (
        <Callout kind="warning">
          <AlertTriangle size={12} /> Outside the sRGB gamut — clamped for display.
        </Callout>
      )}
      <dl className="kv mono">
        {formats.map(([k, v]) => (
          <Fragment key={k}>
            <dt>{k}</dt>
            <dd className="row-sm">
              <span className="grow">{v}</span>
              {v && <CopyButton text={v} />}
            </dd>
          </Fragment>
        ))}
        <dt>Luminance</dt>
        <dd>{r.relativeLuminance}</dd>
      </dl>
      {r.contrast && (
        <>
          <div className="util-contrast" style={{ background: r.contrast.against, color: r.hex }}>
            <span style={{ fontSize: 18, fontWeight: 600 }}>Large text Aa</span>
            <span>Normal body text on {r.contrast.against}</span>
          </div>
          <div className="row wrap">
            <strong>{r.contrast.ratio}:1</strong>
            <Pass ok={r.contrast.aaNormalText} label="AA" />
            <Pass ok={r.contrast.aaLargeText} label="AA large" />
            <Pass ok={r.contrast.aaaNormalText} label="AAA" />
            <Pass ok={r.contrast.aaaLargeText} label="AAA large" />
            <Pass ok={r.contrast.uiComponents} label="UI 3:1" />
          </div>
        </>
      )}
    </div>
  );
}
