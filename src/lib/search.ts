/**
 * Subsequence matching for the command palette.
 *
 * Characters must appear in order but need not be adjacent, so "sfa" finds
 * "SELECT * FROM authors". Scoring favours matches that start at a word
 * boundary and that stay tightly packed, which is what makes the obvious
 * candidate land at the top rather than an incidental scattering of letters.
 */
export interface FuzzyMatch {
  score: number;
  /** Indices in `text` that matched, for highlighting. */
  indices: number[];
}

export function fuzzyMatch(text: string, query: string): FuzzyMatch | null {
  if (!query) return { score: 0, indices: [] };

  const haystack = text.toLowerCase();
  const needle = query.toLowerCase();

  const indices: number[] = [];
  let score = 0;
  let cursor = 0;
  let lastMatch = -1;

  for (const ch of needle) {
    const found = haystack.indexOf(ch, cursor);
    if (found === -1) return null;

    // Consecutive characters are far more likely to be the intended match
    // than the same letters scattered across the string.
    if (found === lastMatch + 1) score += 8;

    // A match at the start of a word usually means the user typed an initial.
    const prev = found > 0 ? haystack[found - 1]! : ' ';
    if (found === 0 || prev === ' ' || prev === '_' || prev === '.' || prev === '-') {
      score += 6;
    }

    // Later matches are weaker, but never negative — a late match still beats
    // no match, and letting the score go negative would rank it below one.
    score += Math.max(0, 4 - Math.floor(found / 12));

    indices.push(found);
    lastMatch = found;
    cursor = found + 1;
  }

  // Prefer shorter candidates: matching 3 of 8 characters is a better signal
  // than matching 3 of 200.
  score += Math.max(0, 20 - Math.floor(text.length / 4));

  return { score, indices };
}

/** Rank items by their best fuzzy match, dropping those that do not match. */
export function fuzzyRank<T>(
  items: T[],
  query: string,
  key: (item: T) => string,
): { item: T; match: FuzzyMatch }[] {
  if (!query.trim()) {
    return items.map((item) => ({ item, match: { score: 0, indices: [] } }));
  }
  return items
    .map((item) => {
      const match = fuzzyMatch(key(item), query);
      return match ? { item, match } : null;
    })
    .filter((x): x is { item: T; match: FuzzyMatch } => x !== null)
    .sort((a, b) => b.match.score - a.match.score);
}

/** Collapse whitespace so multi-line SQL fits on one row. */
export function oneLine(sql: string, max = 120): string {
  const flat = sql.replace(/\s+/g, ' ').trim();
  return flat.length > max ? `${flat.slice(0, max)}…` : flat;
}

/**
 * Relative time for history rows ("3m ago").
 *
 * The store writes SQLite's `datetime('now')`, which is UTC without a zone
 * marker. Appending `Z` stops the browser reading it as local time, which
 * would show every entry hours off.
 */
export function relativeTime(sqliteTimestamp: string, now: Date = new Date()): string {
  const iso = sqliteTimestamp.includes('T')
    ? sqliteTimestamp
    : `${sqliteTimestamp.replace(' ', 'T')}Z`;
  const then = new Date(iso);
  if (Number.isNaN(then.getTime())) return sqliteTimestamp;

  const seconds = Math.floor((now.getTime() - then.getTime()) / 1000);
  if (seconds < 5) return 'just now';
  if (seconds < 60) return `${seconds}s ago`;

  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;

  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;

  return then.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}
