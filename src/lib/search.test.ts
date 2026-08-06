import { describe, expect, it } from 'vitest';

import { fuzzyMatch, fuzzyRank, oneLine, relativeTime } from './search';

describe('fuzzyMatch', () => {
  it('matches a scattered subsequence', () => {
    expect(fuzzyMatch('SELECT * FROM authors', 'sfa')).not.toBeNull();
  });

  it('rejects characters that are out of order', () => {
    expect(fuzzyMatch('abc', 'cba')).toBeNull();
  });

  it('rejects a character that is absent', () => {
    expect(fuzzyMatch('abc', 'abz')).toBeNull();
  });

  it('is case insensitive', () => {
    expect(fuzzyMatch('Daily Report', 'dr')).not.toBeNull();
    expect(fuzzyMatch('daily report', 'DR')).not.toBeNull();
  });

  it('returns the matched indices for highlighting', () => {
    const m = fuzzyMatch('abcd', 'ac');
    expect(m!.indices).toEqual([0, 2]);
  });

  it('scores a contiguous match above a scattered one', () => {
    const contiguous = fuzzyMatch('users', 'use')!;
    const scattered = fuzzyMatch('u_s_e_r', 'use')!;
    expect(contiguous.score).toBeGreaterThan(scattered.score);
  });

  it('scores word-boundary initials well', () => {
    const initials = fuzzyMatch('daily active users', 'dau')!;
    const buried = fuzzyMatch('xxdxxaxxu', 'dau')!;
    expect(initials.score).toBeGreaterThan(buried.score);
  });

  it('prefers a shorter candidate over a long one', () => {
    const short = fuzzyMatch('users', 'users')!;
    const long = fuzzyMatch(`users ${'x'.repeat(300)}`, 'users')!;
    expect(short.score).toBeGreaterThan(long.score);
  });

  it('treats an empty query as a neutral match', () => {
    const m = fuzzyMatch('anything', '');
    expect(m).toEqual({ score: 0, indices: [] });
  });

  it('never produces a negative score for a late match', () => {
    // A late match must still outrank no match at all.
    const m = fuzzyMatch(`${'x'.repeat(400)}q`, 'q')!;
    expect(m.score).toBeGreaterThanOrEqual(0);
  });
});

describe('fuzzyRank', () => {
  const items = ['authors', 'books', 'book_stores', 'access_log'];

  it('returns everything unfiltered for an empty query', () => {
    expect(fuzzyRank(items, '', (s) => s)).toHaveLength(4);
    expect(fuzzyRank(items, '   ', (s) => s)).toHaveLength(4);
  });

  it('drops non-matching items', () => {
    const out = fuzzyRank(items, 'zzz', (s) => s);
    expect(out).toHaveLength(0);
  });

  it('ranks the best match first', () => {
    const out = fuzzyRank(items, 'books', (s) => s);
    expect(out[0]!.item).toBe('books');
  });

  it('ranks an exact prefix above an incidental subsequence', () => {
    const out = fuzzyRank(items, 'acc', (s) => s);
    expect(out[0]!.item).toBe('access_log');
  });
});

describe('oneLine', () => {
  it('collapses newlines and runs of whitespace', () => {
    expect(oneLine('SELECT *\n  FROM   t')).toBe('SELECT * FROM t');
  });

  it('truncates past the limit with an ellipsis', () => {
    const out = oneLine('a'.repeat(200), 10);
    expect(out).toBe(`${'a'.repeat(10)}…`);
  });

  it('leaves short text alone', () => {
    expect(oneLine('SELECT 1', 100)).toBe('SELECT 1');
  });
});

describe('relativeTime', () => {
  // The store writes SQLite's `datetime('now')`, which is UTC with no zone
  // marker. Reading it as local time would shift every entry by the offset.
  const now = new Date('2026-08-05T12:00:00Z');

  it('reads a SQLite timestamp as UTC', () => {
    expect(relativeTime('2026-08-05 11:58:00', now)).toBe('2m ago');
  });

  it('scales the unit with the gap', () => {
    expect(relativeTime('2026-08-05 11:59:30', now)).toBe('30s ago');
    expect(relativeTime('2026-08-05 11:45:00', now)).toBe('15m ago');
    expect(relativeTime('2026-08-05 09:00:00', now)).toBe('3h ago');
    expect(relativeTime('2026-08-03 12:00:00', now)).toBe('2d ago');
  });

  it("says 'just now' for the last few seconds", () => {
    expect(relativeTime('2026-08-05 11:59:59', now)).toBe('just now');
  });

  it('falls back to a date beyond a week', () => {
    const out = relativeTime('2026-07-01 12:00:00', now);
    expect(out).not.toContain('ago');
    expect(out).toMatch(/Jul/);
  });

  it('returns the raw string when it cannot be parsed', () => {
    expect(relativeTime('not a date', now)).toBe('not a date');
  });
});
