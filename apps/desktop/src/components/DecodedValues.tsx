import { useEffect, useMemo, useState } from 'react';
import { AlertTriangle, KeyRound } from 'lucide-react';
import { Empty } from '@/components/ui/misc';
import { api } from '@/lib/ipc';
import { findValues, type Detection } from '@/lib/detect';
import { useNav } from '@/store/nav';

/** JWTs and timestamps found in `text` (headers + body of an exchange, a log line…). */
export function useDetections(text: string | null | undefined): Detection[] {
  return useMemo(() => (text ? findValues(text) : []), [text]);
}

/** `name: value` lines plus the body — what detection scans for an HTTP message. */
export function messageText(headers: [string, string][] | Record<string, string> | null | undefined, body?: string | null): string {
  const list = Array.isArray(headers) ? headers : Object.entries(headers ?? {});
  return `${list.map(([k, v]) => `${k}: ${v}`).join('\n')}\n${body ?? ''}`;
}

/**
 * Values found in a message, decoded locally with the utilities registry (JWT header/claims with
 * expiry warnings, timestamps as dates). Shared by the HTTP response panel, the traffic inspector,
 * webhook events, the log viewer and the mock request journal.
 */
export function DecodedValues({ items, empty, className = 'p-5 col-lg' }: { items: Detection[]; empty?: boolean; className?: string }) {
  const [out, setOut] = useState<Record<string, string>>({});
  const go = useNav((s) => s.go);
  useEffect(() => {
    let live = true;
    Promise.all(items.map((d) => api.utilRun(d.op, d.value).then((r) => [d.value, r] as const, (e) => [d.value, String(e)] as const))).then((pairs) => {
      if (live) setOut(Object.fromEntries(pairs));
    });
    return () => {
      live = false;
    };
  }, [items]);
  if (!items.length)
    return empty ? (
      <Empty icon={<KeyRound />} title="Nothing to decode">
        No JWTs or epoch timestamps in this message.
      </Empty>
    ) : null;
  return (
    <div className={className}>
      {items.map((d) => {
        const decoded = out[d.value];
        let warnings: string[] = [];
        try {
          warnings = d.kind === 'jwt' && decoded ? (JSON.parse(decoded).warnings ?? []) : [];
        } catch {
          /* not JSON */
        }
        return (
          <div key={d.value} className="card col">
            <div className="row">
              <span className="badge accent">{d.label}</span>
              <span className="mono xs truncate grow">{d.value}</span>
              <button className="btn btn-sm btn-ghost" onClick={() => go('utilities', `${d.op}|${d.value}`)}>
                Open in Utilities
              </button>
            </div>
            {warnings.map((w) => (
              <div key={w} className="row-sm text-warning small">
                <AlertTriangle size={13} /> {w}
              </div>
            ))}
            {decoded && <pre className="code">{decoded}</pre>}
          </div>
        );
      })}
    </div>
  );
}

/** Tab label: “Decoded (n)”. */
export function DecodedLabel({ count }: { count: number }) {
  return (
    <>
      <KeyRound size={13} /> Decoded ({count})
    </>
  );
}
