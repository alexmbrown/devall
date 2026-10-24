import { beforeEach, describe, expect, it } from 'vitest';
import { protoKindOf, type ProtoEvent } from '@/lib/api/proto';
import { hexDump, prettyBody } from './format';
import { beginConnect, connected, connectFailed, liveKey, useLive } from './store';

const at = 1_700_000_000_000;

describe('format', () => {
  it('pretty-prints JSON and leaves other text alone', () => {
    expect(prettyBody('{"a":1}')).toEqual({ text: '{\n  "a": 1\n}', json: true });
    expect(prettyBody('[1]').json).toBe(true);
    expect(prettyBody('{nope')).toEqual({ text: '{nope', json: false });
    expect(prettyBody('hello').json).toBe(false);
  });

  it('dumps binary as hex', () => {
    expect(hexDump('AAEC/w==')).toBe(`000000  00 01 02 ff${' '.repeat(36)}  ....`);
    expect(hexDump('%%%')).toBe('');
  });
});

describe('protoKindOf', () => {
  it('maps extensions to protocols', () => {
    expect(protoKindOf('a/b.ws.yaml')).toBe('ws');
    expect(protoKindOf('b.grpc.yaml')).toBe('grpc');
    expect(protoKindOf('b.mqtt.yaml')).toBe('mqtt');
    expect(protoKindOf('b.sio.yaml')).toBe('socketio');
    expect(protoKindOf('b.req.yaml')).toBeNull();
  });
});

describe('live store', () => {
  const key = liveKey('/p', 'chat.ws.yaml');
  beforeEach(() => useLive.setState({ live: {} }));

  it('follows opened/closed events and keeps a log', () => {
    beginConnect(key, 'ws');
    expect(useLive.getState().live[key].status).toBe('connecting');
    const events: ProtoEvent[] = [
      { type: 'connecting', at, target: 'ws://x' },
      { type: 'opened', at, detail: 'HTTP 101' },
      { type: 'message', at, direction: 'in', body: 'hi', binary: false, size: 2 },
    ];
    useLive.getState().append(key, events);
    connected(key, 'ws', 'S1');
    let s = useLive.getState().live[key];
    expect(s.status).toBe('open');
    expect(s.sessionId).toBe('S1');
    expect(s.log.map((e) => e.event.type)).toEqual(['connecting', 'opened', 'message']);

    useLive.getState().append(key, [{ type: 'closed', at, code: 1000 }]);
    s = useLive.getState().live[key];
    expect(s.status).toBe('closed');
    expect(s.sessionId).toBeUndefined();

    useLive.getState().clear(key);
    expect(useLive.getState().live[key].log).toEqual([]);
  });

  it('does not resurrect a session that closed before connect resolved', () => {
    beginConnect(key, 'mqtt');
    useLive.getState().append(key, [{ type: 'closed', at, reason: 'bye' }]);
    connected(key, 'mqtt', 'S2');
    expect(useLive.getState().live[key].sessionId).toBeUndefined();
  });

  it('records connect errors and moves state on rename', () => {
    beginConnect(key, 'socketio');
    connectFailed(key, 'socketio', 'refused');
    expect(useLive.getState().live[key]).toMatchObject({ status: 'closed', error: 'refused' });
    const to = liveKey('/p', 'renamed.sio.yaml');
    useLive.getState().rekey(key, to);
    expect(useLive.getState().live[key]).toBeUndefined();
    expect(useLive.getState().live[to].error).toBe('refused');
  });
});
