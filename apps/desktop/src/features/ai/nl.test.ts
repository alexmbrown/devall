import { describe, expect, it } from 'vitest';
import { explicitWrite, mongoFields } from './nl';

describe('NL → query helpers', () => {
  it('only treats explicit change requests as writes', () => {
    expect(explicitWrite('top 10 customers by revenue')).toBe(false);
    expect(explicitWrite('which orders were updated today?')).toBe(false);
    expect(explicitWrite('delete the test users')).toBe(true);
    expect(explicitWrite('Update the price of SKU A1 to 9.99')).toBe(true);
    expect(explicitWrite('set status to shipped for order 42')).toBe(true);
    expect(explicitWrite('insert a demo customer')).toBe(true);
  });

  it('maps find suggestions to the workspace fields', () => {
    expect(mongoFields({ filter: '', projection: '', sort: '{"a": 1}', limit: 0 })).toEqual({ filter: '{}', projection: '', sort: '{"a": 1}', limit: undefined });
    expect(mongoFields({ filter: '{"x": 1}', projection: '', sort: '', limit: 20 }).limit).toBe(20);
  });
});
