import { describe, expect, it } from 'vitest';
import { describeMessage, isTopicFilter, templateFor, validateDefinition, type MessageEvent } from './protocols';

const msg = (m: Partial<MessageEvent>): MessageEvent => ({
  seq: 1,
  at: 0,
  protocol: 'websocket',
  connection: 'ws-1',
  direction: 'in',
  channel: '/ws',
  body: '',
  binary: false,
  stub: null,
  meta: null,
  ...m,
});

describe('protocol definitions', () => {
  it('starter templates are valid', () => {
    for (const p of ['websocket', 'grpc', 'mqtt'] as const) {
      expect(validateDefinition(p, templateFor(p).content)).toEqual([]);
    }
    expect(templateFor('graphql').name).toBe('schema.graphql');
    expect(templateFor('grpc', 'proto').content).toContain('syntax = "proto3"');
  });

  it('flags what the engine would refuse', () => {
    expect(validateDefinition('websocket', '{"name": "x"}')).toEqual(['request is required (e.g. {"urlPath": "/ws"})']);
    expect(validateDefinition('websocket', '{oops')[0]).toMatch(/^Invalid JSON/);
    expect(validateDefinition('grpc', '{"method": "NoSlash", "response": {"message": {}}}')).toEqual(['method must look like "package.Service/Method"']);
    expect(validateDefinition('grpc', '{"method": "a.B/C", "response": {}}')).toEqual(['response needs a message, messages or a non-OK status']);
    expect(validateDefinition('grpc', '{"method": "a.B/C", "response": {"status": "NOT_FOUND"}}')).toEqual([]);
    expect(validateDefinition('mqtt', '{"rules": [{"topic": "a/#/b"}]}')).toEqual(['rules[0].topic must be a topic filter']);
    expect(validateDefinition('websocket', '{"stubs": [{"request": {}}, {"name": "x"}]}')).toEqual(['#2: request is required (e.g. {"urlPath": "/ws"})']);
    expect(validateDefinition('mqtt', '{"behaviours": [{"rules": "nope"}]}')).toEqual(['rules must be an array']);
  });

  it('checks MQTT topic filters', () => {
    expect(isTopicFilter('a/+/c')).toBe(true);
    expect(isTopicFilter('#')).toBe(true);
    expect(isTopicFilter('a/#')).toBe(true);
    expect(isTopicFilter('a/#/c')).toBe(false);
    expect(isTopicFilter('a+/b')).toBe(false);
    expect(isTopicFilter('')).toBe(false);
  });

  it('describes journal entries', () => {
    expect(describeMessage(msg({ body: 'ping' }))).toBe('ping');
    expect(describeMessage(msg({ body: 'AAAA', binary: true }))).toBe('<3 bytes>');
    expect(describeMessage(msg({ direction: 'event', meta: { event: 'close', code: 4000, reason: 'bye', by: 'mock' } }))).toBe('closed (4000 bye) by mock');
    expect(describeMessage(msg({ protocol: 'grpc', direction: 'event', meta: { event: 'status', status: 5, statusName: 'NOT_FOUND', message: 'no item' } }))).toBe('status NOT_FOUND — no item');
    expect(describeMessage(msg({ protocol: 'mqtt', direction: 'event', meta: { event: 'subscribe', qos: 1 } }))).toBe('subscribed (QoS 1)');
    expect(describeMessage(msg({ direction: 'event', meta: { event: 'open' } }))).toBe('open');
  });
});
