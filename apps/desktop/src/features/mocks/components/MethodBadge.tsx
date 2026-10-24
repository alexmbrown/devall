import clsx from 'clsx';
import type { MethodPattern } from '@/features/mocks/lib/types';
import { methodLabel, methodSetOf } from '@/features/mocks/lib/stub-utils';

/** Method badge that also renders WireMock 4 method sets (`{oneOf: […]}` / `{noneOf: […]}`). */
export function MethodBadge({ method }: { method?: MethodPattern }) {
  const set = methodSetOf(method);
  const m = methodLabel(method);
  // A one-element `oneOf` keeps its method colour; other sets are neutral.
  const single = set ? (set.kind === 'oneOf' && set.methods.length === 1 ? set.methods[0].toUpperCase() : '') : m;
  const cls = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].includes(single) ? `method-${single}` : 'method-other';
  const title = set ? `${set.kind === 'oneOf' ? 'One of' : 'None of'}: ${set.methods.join(', ')}` : m;
  // Keep sets narrow enough for list columns: PUT|PATCH, !GET+2.
  const label = set && m.length > 9 ? `${set.kind === 'noneOf' ? '!' : ''}${set.methods[0]}+${set.methods.length - 1}` : m === 'OPTIONS' ? 'OPT' : m;
  return (
    <span className={clsx('badge method', cls)} title={title}>
      {label}
    </span>
  );
}
