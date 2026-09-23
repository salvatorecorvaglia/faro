import { describe, expect, it } from 'vitest';

import { formatSql } from '@/lib/format';

describe('formatSql', () => {
  it('expands a query onto readable lines', () => {
    const out = formatSql('select a,b from t where a=1', 'postgres');
    expect(out).toContain('SELECT');
    expect(out).toContain('FROM');
    expect(out.split('\n').length).toBeGreaterThan(1);
  });

  it('uppercases keywords but leaves identifiers alone', () => {
    const out = formatSql('select myColumn from myTable', 'postgres');
    expect(out).toContain('SELECT');
    expect(out).toContain('myColumn');
    expect(out).toContain('myTable');
  });

  it('returns unparseable input unchanged rather than throwing', () => {
    // Half-typed SQL is the editor's normal state; mangling it would be worse
    // than doing nothing.
    const partial = 'SELECT * FROM WHERE ((( ';
    expect(() => formatSql(partial, 'postgres')).not.toThrow();
    expect(typeof formatSql(partial, 'postgres')).toBe('string');
  });

  it('leaves empty input alone', () => {
    expect(formatSql('', 'postgres')).toBe('');
    expect(formatSql('   ', 'postgres')).toBe('   ');
  });

  it('formats for engines with their own dialect', () => {
    for (const engine of ['sqlite', 'mysql', 'sqlserver', 'duckdb', null] as const) {
      const out = formatSql('select 1', engine);
      expect(out).toContain('SELECT');
    }
  });

  it('preserves string literal contents', () => {
    const out = formatSql("select 'o''brien; select' from t", 'postgres');
    expect(out).toContain("'o''brien; select'");
  });
});
