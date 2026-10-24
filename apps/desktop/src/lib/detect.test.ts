import { describe, expect, it } from 'vitest';
import { detect, findValues, isEpoch } from './detect';
import { segments } from './template';

const JWT = 'eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiI0MiIsImV4cCI6NDEwMjQ0NDgwMH0.c2lnbmF0dXJl';

describe('detect', () => {
  it('recognizes common values', () => {
    expect(detect(JWT)?.op).toBe('jwt.decode');
    expect(detect(`Bearer ${JWT}`)?.value).toBe(JWT);
    expect(detect('1700000000')?.kind).toBe('timestamp');
    expect(detect('1700000000123')?.kind).toBe('timestamp');
    expect(detect('2026-10-21T12:00:00Z')?.op).toBe('time.convert');
    expect(detect('https://x.io/a?b=1')?.op).toBe('url.parse');
    expect(detect('{"a":1}')?.op).toBe('json.format');
    expect(detect('aGVsbG8gd29ybGQh')?.op).toBe('base64.decode');
    expect(detect('a%20b')?.op).toBe('url.decode');
  });

  it('avoids false positives', () => {
    expect(detect('hello')).toBeNull();
    expect(detect('deadbeefdeadbeef')).toBeNull(); // hex, not base64
    expect(detect('123')).toBeNull();
    expect(isEpoch('0999999999')).toBe(false);
    expect(detect('{not json')).toBeNull();
  });

  it('recognizes certificates, networks, cron expressions and gzip', () => {
    const pem = '-----BEGIN CERTIFICATE-----\nMIIBszCCAVmgAwIBAgIU\n-----END CERTIFICATE-----';
    expect(detect(pem)?.op).toBe('x509.decode');
    expect(detect('10.0.0.0/8')?.op).toBe('cidr.calc');
    expect(detect('2001:db8:abcd::/48')?.op).toBe('cidr.calc');
    expect(detect('192.168.1.10')?.op).toBe('ip.info');
    expect(detect('::1')?.op).toBe('ip.info');
    expect(detect('fe80::1ff:fe23:4567:890a%eth0')?.kind).toBe('ip');
    expect(detect('2001:0db8:0000:0000:0000:ff00:0042:8329')?.kind).toBe('ip');
    expect(detect('*/5 * * * *')?.op).toBe('cron.explain');
    expect(detect('0 9 * * MON-FRI')?.op).toBe('cron.explain');
    expect(detect('0 0 12 ? * WED')?.op).toBe('cron.explain');
    expect(detect('@daily')?.op).toBe('cron.explain');
    expect(detect('H4sIAAAAAAAA//NIzcnJV/DARwIAOz1tpyMAAAA=')?.op).toBe('gzip.decompress');
  });

  it('keeps the new detectors strict', () => {
    expect(detect('12:30:45')).toBeNull(); // a time, not IPv6
    expect(detect('999.1.1.1')).toBeNull();
    expect(detect('10.0.0.0/33')).toBeNull();
    expect(detect('1 2 3 4 5')).toBeNull(); // numbers, not cron
    expect(detect('the cat sat on mats')).toBeNull();
    expect(detect('-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----')).toBeNull();
  });

  it('finds values inside text', () => {
    const body = JSON.stringify({ token: JWT, exp: 1700000000, n: 1234567890123456 });
    const found = findValues(`Authorization: Bearer ${JWT}\n${body}`);
    expect(found.map((f) => f.kind)).toEqual(['jwt', 'timestamp']);
  });
});

describe('template segments', () => {
  it('splits text and {{references}}', () => {
    expect(segments('{{baseUrl}}/users/{{ id }}?x={{')).toEqual([
      { kind: 'var', text: '{{baseUrl}}', name: 'baseUrl' },
      { kind: 'text', text: '/users/' },
      { kind: 'var', text: '{{ id }}', name: 'id' },
      { kind: 'text', text: '?x={{' },
    ]);
  });
});
