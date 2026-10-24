import { describe, expect, it } from 'vitest';
import { sourceTarget } from './sourceTarget';

describe('cited source targets', () => {
  it('maps references to where they open', () => {
    expect(sourceTarget({ kind: 'mockStub', source: 'mocks/mappings/orders.json#7c6b' })).toEqual({ type: 'stub', file: 'mappings/orders.json', id: '7c6b' });
    expect(sourceTarget({ kind: 'mockNearMiss', source: '#abc' })).toEqual({ type: 'none' });
    expect(sourceTarget({ kind: 'event', source: 'event:01JABC' })).toEqual({ type: 'event', id: '01JABC' });
    expect(sourceTarget({ kind: 'podEvents', source: 'pod:gke_p_z_c/shop/orders-7d9' })).toEqual({ type: 'pod', context: 'gke_p_z_c', namespace: 'shop', name: 'orders-7d9' });
    expect(sourceTarget({ kind: 'podLogs', source: 'pod:arn:aws:eks:eu/c1/shop/api-0' })).toEqual({ type: 'pod', context: 'arn:aws:eks:eu/c1', namespace: 'shop', name: 'api-0' });
    expect(sourceTarget({ kind: 'note', source: 'notes/design/auth.md' })).toEqual({ type: 'note', path: 'design/auth.md' });
    expect(sourceTarget({ kind: 'exchange', source: 'request' })).toEqual({ type: 'request' });
  });
});
