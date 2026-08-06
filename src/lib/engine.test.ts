import { describe, expect, it } from 'vitest';

import { editorPlaceholder, isSqlEngine, starterQuery } from './engine';

describe('isSqlEngine', () => {
  it('is false only for MongoDB', () => {
    expect(isSqlEngine('mongodb')).toBe(false);
    for (const engine of [
      'postgres',
      'mysql',
      'mariadb',
      'sqlite',
      'sqlserver',
      'duckdb',
      'clickhouse',
      'cockroachdb',
      'redshift',
    ] as const) {
      expect(isSqlEngine(engine)).toBe(true);
    }
  });

  it('treats an unknown connection as SQL', () => {
    // The SQL path is the safe default: it never silently sends a document
    // query to a relational engine.
    expect(isSqlEngine(null)).toBe(true);
  });
});

describe('editorPlaceholder', () => {
  it('shows Mongo syntax for Mongo and SQL otherwise', () => {
    expect(editorPlaceholder('mongodb')).toContain('db.collection.find');
    expect(editorPlaceholder('postgres')).toContain('SQL');
  });
});

describe('starterQuery', () => {
  it('seeds a Mongo tab with a runnable query', () => {
    // MongoDB syntax is not guessable, so a new tab shows the shape.
    expect(starterQuery('mongodb')).toBe('db.collection.find({})');
    expect(starterQuery('mongodb', 'authors')).toBe('db.authors.find({})');
  });

  it('leaves a SQL tab empty', () => {
    expect(starterQuery('postgres')).toBe('');
  });
});
