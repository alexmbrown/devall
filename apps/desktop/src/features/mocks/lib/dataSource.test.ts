import { describe, expect, it } from 'vitest';
import { dataSourceSnippet, lookupConfig } from './dataSource';

describe('data source lookups', () => {
  it('builds helper snippets', () => {
    expect(dataSourceSnippet('users', 'id', 'request.path.id')).toBe("{{dataSource 'users' 'id' request.path.id}}");
    expect(dataSourceSnippet('shop.orders', 'user_id', "(jsonPath request.body '$.id')")).toBe("{{dataSource 'shop.orders' 'user_id' (jsonPath request.body '$.id')}}");
    expect(dataSourceSnippet('users', 'role', 'admin')).toBe("{{dataSource 'users' 'role' 'admin'}}");
    expect(dataSourceSnippet('users', 'id', '42')).toBe("{{dataSource 'users' 'id' 42}}");
    expect(dataSourceSnippet("o'neil", '', '')).toBe("{{dataRows 'o\\'neil'}}");
  });

  it('builds transformerParameters.dataSource', () => {
    expect(lookupConfig('users', 'id', 'request.path.id', true)).toEqual({ name: 'users', where: { id: '{{request.path.id}}' }, notFoundStatus: 404 });
    expect(lookupConfig('users', 'id', '{{request.query.id}}', false)).toEqual({ name: 'users', where: { id: '{{request.query.id}}' } });
  });
});
