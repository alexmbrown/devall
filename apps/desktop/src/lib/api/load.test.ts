import { describe, expect, it } from 'vitest';
import { cloudTargetFor, type CloudTarget } from './load';

const target = (over: Partial<CloudTarget>): CloudTarget => ({
  key: 'api.example.com:443',
  origin: 'https://api.example.com',
  hostname: 'api.example.com',
  port: 443,
  status: 'verified',
  token: 'devall-load-x',
  fileUrl: 'https://api.example.com/.well-known/devall-load-verification.txt',
  dnsName: '_devall-load.api.example.com',
  verifiedAt: null,
  expiresAt: null,
  method: 'http',
  lastError: null,
  ...over,
});

describe('cloudTargetFor', () => {
  it('matches host and port, or a DNS verification of the host', () => {
    const https = target({});
    const local = target({ key: '127.0.0.1:3000', origin: 'http://127.0.0.1:3000', hostname: '127.0.0.1', port: 3000 });
    expect(cloudTargetFor([https, local], 'https://API.example.com')).toBe(https);
    expect(cloudTargetFor([https, local], 'http://127.0.0.1:3000')).toBe(local);
    expect(cloudTargetFor([https], 'http://api.example.com')).toBeUndefined();
    const dns = target({ method: 'dns' });
    expect(cloudTargetFor([dns], 'http://api.example.com:8080')).toBe(dns);
    expect(cloudTargetFor([https], 'not a url')).toBeUndefined();
  });
});
