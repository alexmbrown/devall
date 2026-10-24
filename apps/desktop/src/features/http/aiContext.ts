import type { ResponseContext } from '@/lib/api/ai';
import type { SendResult } from '@/lib/types';

/** The exchange the AI features see for "explain this response" / "suggest tests" (values are redacted server-side). */
export function responseContext(result: SendResult | undefined | null): ResponseContext | null {
  const res = result?.response;
  if (!result || !res) return null;
  return {
    method: result.sent?.method,
    url: result.sent?.url,
    requestHeaders: result.sent?.headers,
    requestBody: result.sent?.body,
    status: res.status,
    statusText: res.statusText,
    headers: res.headers,
    body: res.bodyText ?? '',
    durationMs: res.timings.totalMs,
    failures: result.tests.filter((t) => !t.passed).map((t) => (t.message ? `${t.label}: ${t.message}` : t.label)),
  };
}
