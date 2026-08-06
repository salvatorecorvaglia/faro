import { format, type SqlLanguage } from 'sql-formatter';

import type { Engine } from '@/ipc/types';

/**
 * Pretty-print SQL.
 *
 * Returns the input unchanged when the formatter cannot parse it. A half-typed
 * query is the normal state of an editor, and mangling it — or throwing — would
 * be worse than leaving it alone.
 */
export function formatSql(sql: string, engine: Engine | null): string {
  if (!sql.trim()) return sql;
  try {
    return format(sql, {
      language: languageFor(engine),
      keywordCase: 'upper',
      indentStyle: 'standard',
      linesBetweenQueries: 2,
      tabWidth: 2,
    });
  } catch {
    return sql;
  }
}

/** Map an engine onto the closest dialect sql-formatter knows. */
function languageFor(engine: Engine | null): SqlLanguage {
  switch (engine) {
    // Cockroach speaks the Postgres wire protocol and its SQL dialect too.
    case 'postgres':
    case 'cockroachdb':
      return 'postgresql';
    case 'redshift':
      return 'redshift';
    case 'mysql':
      return 'mysql';
    case 'mariadb':
      return 'mariadb';
    case 'sqlite':
      return 'sqlite';
    case 'sqlserver':
      return 'transactsql';
    case 'duckdb':
      return 'duckdb';
    case 'clickhouse':
      // Not a dialect sql-formatter models; generic SQL is the safe fallback.
      return 'sql';
    default:
      return 'sql';
  }
}
