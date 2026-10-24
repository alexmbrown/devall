import type { CSSProperties } from 'react';
import clsx from 'clsx';
import { envChipInfo } from '@/lib/envColor';
import type { EnvironmentFile } from '@/lib/types';

/**
 * Environment chip: a preset type (LOCAL / DEV / TEST / STAGING / PROD, as in wiremock-ui's server
 * switcher) or a custom colour with the environment's own short label. A custom colour without a
 * label renders as a dot.
 */
export function EnvChip({ env, className }: { env: Pick<EnvironmentFile, 'color' | 'label'>; className?: string }) {
  const info = envChipInfo(env);
  if (!info) return null;
  const style = info.color ? ({ '--env-c': info.color } as CSSProperties) : undefined;
  if (!info.text) return <span className={clsx('env-dot', className)} style={style} aria-hidden />;
  return (
    <span className={clsx('env-chip', info.preset ? `env-${info.preset}` : info.color ? 'env-custom' : 'env-other', className)} style={style}>
      {info.text}
    </span>
  );
}
