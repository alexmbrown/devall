import { describe, expect, it } from 'vitest';
import { quoteIdent, selectSql, sqlLiteral, toInsert } from './format';

describe('SQL text per dialect', () => {
  it('quotes identifiers', () => {
    expect(quoteIdent('we"ird', 'postgres')).toBe('"we""ird"');
    expect(quoteIdent('we`ird', 'mysql')).toBe('`we``ird`');
    expect(quoteIdent('we]ird', 'sqlserver')).toBe('[we]]ird]');
  });

  it('writes literals', () => {
    expect(sqlLiteral(true, 'postgres')).toBe('TRUE');
    expect(sqlLiteral(true, 'sqlserver')).toBe('1');
    expect(sqlLiteral("O'Brien", 'sqlserver')).toBe("N'O''Brien'");
    expect(sqlLiteral(null, 'sqlserver')).toBe('NULL');
  });

  it('limits listings the way each dialect does', () => {
    expect(selectSql('*', '[dbo].[t]', 100, 'sqlserver')).toBe('SELECT TOP (100) *\nFROM [dbo].[t];\n');
    expect(selectSql('*', '"t"', 100, 'postgres')).toBe('SELECT *\nFROM "t"\nLIMIT 100;\n');
    expect(toInsert('[t]', [{ name: 'a', typeName: 'nvarchar', kind: 'text' }], [['x']], 'sqlserver')).toBe("INSERT INTO [t] ([a]) VALUES (N'x');\n");
  });
});
