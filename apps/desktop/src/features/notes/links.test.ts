import { describe, expect, it } from 'vitest';
import { resolveProjectLink } from './links';

describe('resolveProjectLink', () => {
  it('opens requests and notes', () => {
    expect(resolveProjectLink('http/users/list.req.yaml')).toEqual({ kind: 'request', path: 'users/list.req.yaml' });
    expect(resolveProjectLink('/http/chat.ws.yaml#x')).toEqual({ kind: 'request', path: 'chat.ws.yaml' });
    expect(resolveProjectLink('notes/design/auth.md')).toEqual({ kind: 'section', section: 'notes', param: 'design/auth.md' });
    expect(resolveProjectLink('runbook.md')).toEqual({ kind: 'section', section: 'notes', param: 'runbook.md' });
  });

  it('resolves relative links against the note', () => {
    expect(resolveProjectLink('other.md', 'design/auth.md')).toEqual({ kind: 'section', section: 'notes', param: 'design/other.md' });
    expect(resolveProjectLink('../ops/runbook.ipynb', 'design/auth.md')).toEqual({ kind: 'section', section: 'notes', param: 'ops/runbook.ipynb' });
    expect(resolveProjectLink('../../http/login.req.yaml', 'design/auth.md')).toEqual({ kind: 'request', path: 'login.req.yaml' });
    expect(resolveProjectLink('../../../etc/passwd', 'design/auth.md')).toEqual({ kind: 'unknown', path: '../../../etc/passwd' });
    // Root folders win over the note's folder.
    expect(resolveProjectLink('mocks/mappings/orders.json', 'design/auth.md')).toEqual({ kind: 'stub', file: 'mappings/orders.json' });
  });

  it('routes the other project folders', () => {
    expect(resolveProjectLink('mocks/__files/a.json')).toEqual({ kind: 'section', section: 'mocks', param: 'files' });
    expect(resolveProjectLink('webhooks/stripe.webhook.yaml')).toEqual({ kind: 'section', section: 'webhooks', param: 'stripe' });
    expect(resolveProjectLink('load/checkout.load.yaml')).toEqual({ kind: 'section', section: 'load', param: 'checkout.load.yaml' });
    expect(resolveProjectLink('load/runs/checkout/1.json')).toEqual({ kind: 'section', section: 'load', param: undefined });
    expect(resolveProjectLink('connections/prod/orders.conn.yaml')).toEqual({ kind: 'section', section: 'data', param: 'prod/orders.conn.yaml' });
    expect(resolveProjectLink('queries/orders/top.sql')).toEqual({ kind: 'section', section: 'data', param: 'query:orders/top.sql' });
    expect(resolveProjectLink('data/users.data.yaml')).toEqual({ kind: 'section', section: 'fakedata', param: 'users.data.yaml' });
    expect(resolveProjectLink('environments/staging.env.yaml')).toEqual({ kind: 'section', section: 'environments', param: 'staging' });
    expect(resolveProjectLink('Cargo.toml')).toEqual({ kind: 'unknown', path: 'Cargo.toml' });
    expect(resolveProjectLink('http/%5F%5Fsnapshots__/x.snap.json')).toEqual({ kind: 'section', section: 'http' });
  });
});
