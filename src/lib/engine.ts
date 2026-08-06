import type { Engine } from '@/ipc/types';

/**
 * Whether an engine's query language is SQL.
 *
 * Mirrors `Engine::is_sql` on the Rust side. Kept as a local predicate rather
 * than read from `list_engines` because the editor needs the answer
 * synchronously, on the first render, before any IPC has resolved.
 */
export function isSqlEngine(engine: Engine | null): boolean {
  return engine !== 'mongodb';
}

/** Placeholder text for an empty editor, in the engine's own language. */
export function editorPlaceholder(engine: Engine | null): string {
  return isSqlEngine(engine)
    ? 'Write SQL here…'
    : 'Write a query here, e.g. db.collection.find({})';
}

/** A starter query for a newly opened tab, so the syntax is discoverable. */
export function starterQuery(engine: Engine | null, collection?: string): string {
  if (isSqlEngine(engine)) return '';
  return `db.${collection ?? 'collection'}.find({})`;
}
