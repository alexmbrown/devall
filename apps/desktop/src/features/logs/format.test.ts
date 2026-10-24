import { describe, expect, it } from 'vitest';
import { fieldClause, quoteValue } from '@/lib/api/logs';
import { addClause, chipFields, filterKey, firstLine, fmtCount } from './format';

describe('log viewer helpers', () => {
  it('quotes values like the Rust query parser expects', () => {
    expect(quoteValue('say "hi" \\d')).toBe('"say \\"hi\\" \\\\d"');
    expect(fieldClause('status', 500)).toBe('status=500');
    expect(fieldClause('path', '/api/users')).toBe('path=/api/users');
    expect(fieldClause('msg', 'two words')).toBe('msg="two words"');
    expect(fieldClause('path', '/api/*')).toBe('path="/api/*"');
    expect(fieldClause('ok', true)).toBe('ok=true');
  });

  it('adds clauses without duplicates and keeps OR groups intact', () => {
    expect(addClause('', 'status=500')).toBe('status=500');
    expect(addClause('timeout', 'status=500')).toBe('timeout status=500');
    expect(addClause('timeout status=500', 'status=500')).toBe('timeout status=500');
    expect(addClause('a OR b', 'level=error')).toBe('(a OR b) level=error');
  });

  it('splits messages and formats counts', () => {
    expect(firstLine('one')).toEqual(['one', 0]);
    expect(firstLine('one\ntwo\nthree')).toEqual(['one', 2]);
    expect(fmtCount(999)).toBe('999');
    expect(fmtCount(12_345)).toBe('12k');
    expect(fmtCount(2_500_000)).toBe('2.5M');
  });

  it('picks chip fields, skipping mapped keys and objects', () => {
    const e = {
      index: 0,
      lineNo: 1,
      lineCount: 1,
      ts: 0,
      level: 'info' as const,
      message: 'done',
      fields: { ts: 'x', level: 'info', msg: 'done', status: 200, user: { id: 1 }, path: '/a' },
      raw: '',
      format: 'json' as const,
    };
    expect(chipFields(e).map(([k]) => k)).toEqual(['status', 'path']);
  });

  it('keys filters independent of level order', () => {
    expect(filterKey({ query: ' a ', levels: ['warn', 'error'] })).toBe(filterKey({ query: 'a', levels: ['error', 'warn'], from: null, to: null }));
  });
});
