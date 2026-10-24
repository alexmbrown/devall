import { eventsApi, logsApi, type Filter, type Histogram, type LogEntry, type Page, type PatternResult, type SqlTarget, type TopValues } from '@/lib/api/logs';
import { EVENTS_KEY } from './store';

/** One data API over a log source or the project event store. */
export interface DataApi {
  key: string;
  isEvents: boolean;
  query(filter: Filter, offset: number, limit: number): Promise<Page>;
  histogram(filter: Filter, buckets: number): Promise<Histogram>;
  patterns(filter: Filter): Promise<PatternResult>;
  locate(filter: Filter, ts: number): Promise<number | null>;
  topValues(filter: Filter, field: string): Promise<TopValues>;
  /** Full (untruncated) entry for the detail drawer. */
  detail(e: LogEntry): Promise<LogEntry | null>;
  sqlTarget: SqlTarget;
}

export function dataApi(key: string, root: string | undefined): DataApi | null {
  if (key === EVENTS_KEY) {
    if (!root) return null;
    return {
      key,
      isEvents: true,
      query: (f, o, l) => eventsApi.query(root, f, o, l),
      histogram: (f, b) => eventsApi.histogram(root, f, b),
      patterns: (f) => eventsApi.patterns(root, f),
      locate: (f, ts) => eventsApi.locate(root, f, ts),
      topValues: (f, field) => eventsApi.topValues(root, f, field, 10),
      detail: async (e) => e,
      sqlTarget: { root },
    };
  }
  return {
    key,
    isEvents: false,
    query: (f, o, l) => logsApi.query(key, f, o, l),
    histogram: (f, b) => logsApi.histogram(key, f, b),
    patterns: (f) => logsApi.patterns(key, f),
    locate: (f, ts) => logsApi.locate(key, f, ts),
    topValues: (f, field) => logsApi.topValues(key, f, field, 10),
    detail: (e) => logsApi.entry(key, e.index),
    sqlTarget: root ? { handle: key, root } : { handle: key },
  };
}
