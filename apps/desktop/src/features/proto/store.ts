import { create } from 'zustand';
import { protoApi, type GrpcStatus, type ProtoEvent, type ProtoKind } from '@/lib/api/proto';

/** Connection state of one open file, kept outside the editor so it survives tab switches. */
export type LiveStatus = 'idle' | 'connecting' | 'open' | 'closed';

export interface LogEntry {
  seq: number;
  event: ProtoEvent;
}

export interface LiveState {
  kind: ProtoKind;
  status: LiveStatus;
  /** Session id (WebSocket/MQTT/Socket.IO) or call id (gRPC) while live. */
  sessionId?: string;
  log: LogEntry[];
  /** Final status of the latest gRPC call. */
  grpcStatus?: GrpcStatus;
  /** Last connect error (not an event: the connection never opened). */
  error?: string;
}

const MAX_LOG = 5000;
let seq = 0;

interface LiveStore {
  live: Record<string, LiveState>;
  patch(key: string, kind: ProtoKind, p: Partial<LiveState>): void;
  append(key: string, events: ProtoEvent[]): void;
  clear(key: string): void;
  rekey(from: string, to: string): void;
}

export const liveKey = (root: string, path: string) => `${root}::${path}`;

const blank = (kind: ProtoKind): LiveState => ({ kind, status: 'idle', log: [] });

export const useLive = create<LiveStore>((set) => ({
  live: {},
  patch: (key, kind, p) => set((s) => ({ live: { ...s.live, [key]: { ...(s.live[key] ?? blank(kind)), ...p } } })),
  append: (key, events) =>
    set((s) => {
      const cur = s.live[key];
      if (!cur) return s;
      let status = cur.status;
      let sessionId = cur.sessionId;
      for (const e of events) {
        if (e.type === 'opened') status = 'open';
        if (e.type === 'closed') {
          status = 'closed';
          if (cur.kind !== 'grpc') sessionId = undefined;
        }
      }
      const log = [...cur.log, ...events.map((event) => ({ seq: ++seq, event }))];
      return { live: { ...s.live, [key]: { ...cur, status, sessionId, log: log.length > MAX_LOG ? log.slice(-MAX_LOG) : log } } };
    }),
  clear: (key) => set((s) => (s.live[key] ? { live: { ...s.live, [key]: { ...s.live[key], log: [], grpcStatus: undefined, error: undefined } } } : s)),
  rekey: (from, to) =>
    set((s) => {
      if (!s.live[from] || from === to) return s;
      const { [from]: moved, ...rest } = s.live;
      return { live: { ...rest, [to]: moved } };
    }),
}));

/** The live state of a file (a stable idle state when nothing happened yet). */
export function useLiveState(key: string, kind: ProtoKind): LiveState {
  return useLive((s) => s.live[key]) ?? IDLE[kind];
}

const IDLE: Record<ProtoKind, LiveState> = { ws: blank('ws'), grpc: blank('grpc'), mqtt: blank('mqtt'), socketio: blank('socketio') };

/**
 * An event callback for a session: events arrive faster than React should render, so they are
 * batched per animation frame.
 */
export function eventSink(key: string): (e: ProtoEvent) => void {
  let pending: ProtoEvent[] = [];
  let scheduled = false;
  const flush = () => {
    scheduled = false;
    const batch = pending;
    pending = [];
    if (batch.length) useLive.getState().append(key, batch);
  };
  return (e) => {
    pending.push(e);
    if (!scheduled) {
      scheduled = true;
      // rAF doesn't run in hidden windows; fall back so closing events still land.
      if (typeof document !== 'undefined' && document.hidden) setTimeout(flush, 50);
      else requestAnimationFrame(flush);
    }
  };
}

/** Starts a connection attempt in the store. */
export function beginConnect(key: string, kind: ProtoKind) {
  useLive.getState().patch(key, kind, { status: 'connecting', error: undefined, sessionId: undefined, grpcStatus: undefined });
}

/** Records the session id once `*_connect` resolved (unless it already closed meanwhile). */
export function connected(key: string, kind: ProtoKind, sessionId: string) {
  const cur = useLive.getState().live[key];
  if (cur?.status === 'closed') return;
  useLive.getState().patch(key, kind, { status: 'open', sessionId });
}

export function connectFailed(key: string, kind: ProtoKind, error: string) {
  useLive.getState().patch(key, kind, { status: 'closed', error, sessionId: undefined });
}

/**
 * Closes the live session of a file, if any. Call it when the file's tab closes or the file is
 * deleted.
 */
export async function closeProtoSession(root: string, path: string): Promise<void> {
  const key = liveKey(root, path);
  const cur = useLive.getState().live[key];
  if (!cur?.sessionId) return;
  if (cur.kind === 'grpc') await protoApi.grpcCancel(cur.sessionId).catch(() => false);
  else await protoApi.close(cur.sessionId).catch(() => false);
}
