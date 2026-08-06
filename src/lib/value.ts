import type { ColumnInfo, ResultSet, Value } from '@/ipc/types';

/**
 * Render a cell for the grid.
 *
 * NULL returns an empty string here and is styled distinctly by the cell
 * component — a literal "NULL" text would be indistinguishable from a string
 * whose contents are the word NULL.
 */
export function formatValue(v: Value): string {
  switch (v.kind) {
    case 'null':
      return '';
    case 'bool':
      return v.value ? 'true' : 'false';
    case 'int':
    case 'float':
      return String(v.value);
    case 'decimal':
    case 'text':
    case 'date':
    case 'time':
    case 'timestamp':
    case 'uuid':
      return v.value;
    case 'bytes':
      return `[${formatBytes(v.value.length)}]`;
    case 'json':
      return JSON.stringify(v.value);
    case 'array':
      return `{${v.value.map(formatValue).join(', ')}}`;
    case 'unsupported':
      return `[${v.value}]`;
  }
}

/** Values that are right-aligned in the grid, as numbers conventionally are. */
export function isNumeric(v: Value): boolean {
  return v.kind === 'int' || v.kind === 'float' || v.kind === 'decimal';
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * Convert a value to its JSON equivalent for the JSON result view.
 *
 * Decimals stay strings: turning them into JS numbers would silently round
 * anything beyond 2^53, which defeats the point of carrying them as text.
 * Bytes become a descriptor object rather than a huge numeric array.
 */
export function toJson(v: Value): unknown {
  switch (v.kind) {
    case 'null':
      return null;
    case 'bool':
    case 'int':
    case 'float':
      return v.value;
    case 'decimal':
    case 'text':
    case 'date':
    case 'time':
    case 'timestamp':
    case 'uuid':
    case 'unsupported':
      return v.value;
    case 'json':
      return v.value;
    case 'array':
      return v.value.map(toJson);
    case 'bytes':
      return { $bytes: v.value.length };
  }
}

/** Render a whole result set as an array of objects, for the JSON view. */
export function resultToJson(result: ResultSet): Record<string, unknown>[] {
  return result.rows.map((row) => {
    const obj: Record<string, unknown> = {};
    result.columns.forEach((col, i) => {
      const cell = row[i];
      obj[uniqueKey(obj, col)] = cell ? toJson(cell) : null;
    });
    return obj;
  });
}

/**
 * SQL permits duplicate column names in a result (`SELECT id FROM a JOIN b`),
 * but a JSON object cannot hold both. Suffix collisions rather than dropping
 * data silently.
 */
function uniqueKey(obj: Record<string, unknown>, col: ColumnInfo): string {
  if (!(col.name in obj)) return col.name;
  let n = 2;
  while (`${col.name}_${n}` in obj) n += 1;
  return `${col.name}_${n}`;
}

/** Human-readable elapsed time for the results footer. */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(2)} s`;
  const m = Math.floor(ms / 60_000);
  const s = ((ms % 60_000) / 1000).toFixed(0);
  return `${m}m ${s}s`;
}

export function formatRowCount(n: number): string {
  return n.toLocaleString();
}
