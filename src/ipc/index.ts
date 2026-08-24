/**
 * Typed wrappers around Tauri's `invoke`.
 *
 * Every backend call goes through here, so component code never deals with
 * command-name strings or untyped payloads.
 */
import { invoke } from '@tauri-apps/api/core';

import type {
  ApplyResult,
  BackupFileInfo,
  BackupOptions,
  BackupResult,
  BrowseOptions,
  ConnectionConfig,
  ConnectionStatus,
  EngineInfo,
  ExportFormat,
  ExportOptions,
  FaroError,
  GuardedStatement,
  HistoryEntry,
  ImportOptions,
  ImportPreview,
  PendingChange,
  RestoreOptions,
  RestoreResult,
  ResultSet,
  RunResult,
  SavedQuery,
  SchemaInfo,
  TableColumns,
  TableDetail,
  TableInfo,
  TableRef,
  TransferResult,
} from './types';

export * from './types';

/**
 * Normalize a rejected invoke into a `FaroError`.
 *
 * Tauri rejects with whatever the command's error type serialized to; a panic
 * or a non-command failure comes back as a bare string, so both shapes are
 * handled rather than assuming the tagged object.
 */
export function toFaroError(e: unknown): FaroError {
  if (typeof e === 'object' && e !== null && 'kind' in e && 'message' in e) {
    return e as FaroError;
  }
  return { kind: 'other', message: String(e) };
}

export function errorMessage(e: unknown): string {
  return toFaroError(e).message;
}

// -- Connections -----------------------------------------------------------

export const listConnections = () => invoke<ConnectionConfig[]>('list_connections');

export const listConnectionStatus = () => invoke<ConnectionStatus[]>('list_connection_status');

/**
 * Create or update a connection.
 *
 * Pass `undefined` for `password` to leave a stored password untouched, or an
 * empty string to clear it.
 */
export const saveConnection = (config: ConnectionConfig, password?: string) =>
  invoke<ConnectionConfig>('save_connection', { config, password });

export const deleteConnection = (id: string) => invoke<void>('delete_connection', { id });

export const connect = (id: string) => invoke<void>('connect', { id });

export const disconnect = (id: string) => invoke<void>('disconnect', { id });

export const testConnection = (config: ConnectionConfig, password?: string) =>
  invoke<void>('test_connection', { config, password });

export const connectedIds = () => invoke<string[]>('connected_ids');

export const keychainAvailable = () => invoke<boolean>('keychain_available');

export const listEngines = () => invoke<EngineInfo[]>('list_engines');

// -- Schema ----------------------------------------------------------------

export const listSchemas = (connectionId: string) =>
  invoke<SchemaInfo[]>('list_schemas', { connectionId });

export const listTables = (connectionId: string, schema: string | null) =>
  invoke<TableInfo[]>('list_tables', { connectionId, schema });

export const describeTable = (connectionId: string, table: TableRef) =>
  invoke<TableDetail>('describe_table', { connectionId, table });

export const schemaSnapshot = (connectionId: string, schema: string | null) =>
  invoke<TableColumns[]>('schema_snapshot', { connectionId, schema });

export const browseTable = (
  connectionId: string,
  table: TableRef,
  options: BrowseOptions,
  queryId: string,
) => invoke<ResultSet>('browse_table', { connectionId, table, options, queryId });

// -- Editing ---------------------------------------------------------------

/** The SQL some staged changes would run, without running it. */
export const previewChanges = (connectionId: string, table: TableRef, changes: PendingChange[]) =>
  invoke<GuardedStatement[]>('preview_changes', { connectionId, table, changes });

/** Apply staged changes atomically. All of them commit, or none do. */
export const applyChanges = (connectionId: string, table: TableRef, changes: PendingChange[]) =>
  invoke<ApplyResult>('apply_changes', { connectionId, table, changes });

// -- Import and export -----------------------------------------------------

export const exportResult = (path: string, result: ResultSet, options: ExportOptions) =>
  invoke<TransferResult>('export_result', { path, result, options });

/**
 * Re-reads the table from the database, so it is not capped at one page.
 *
 * `queryId` registers the export with the same cancellation registry a query
 * uses, so `cancelQuery(connectionId, queryId)` can stop it mid-flight.
 */
export const exportTable = (
  connectionId: string,
  table: TableRef,
  path: string,
  options: ExportOptions,
  queryId: string,
) => invoke<TransferResult>('export_table', { connectionId, table, path, options, queryId });

export const previewImport = (path: string, hasHeader: boolean) =>
  invoke<ImportPreview>('preview_import', { path, hasHeader });

export const importFile = (
  connectionId: string,
  table: TableRef,
  path: string,
  options: ImportOptions,
) => invoke<TransferResult>('import_file', { connectionId, table, path, options });

export const suggestedExportName = (base: string, format: ExportFormat) =>
  invoke<string>('suggested_export_name', { base, format });

// -- Backup and restore ----------------------------------------------------

/** Progress arrives on these events rather than in the return value. */
export const BACKUP_PROGRESS_EVENT = 'faro://backup-progress';
export const RESTORE_PROGRESS_EVENT = 'faro://restore-progress';

/** `queryId` lets `cancelQuery` stop a backup mid-flight, the same as a query. */
export const backupDatabase = (
  connectionId: string,
  path: string,
  options: BackupOptions,
  queryId: string,
) => invoke<BackupResult>('backup_database', { connectionId, path, options, queryId });

/** `queryId` lets `cancelQuery` stop a restore mid-flight, the same as a query. */
export const restoreDatabase = (
  connectionId: string,
  path: string,
  options: RestoreOptions,
  queryId: string,
) => invoke<RestoreResult>('restore_database', { connectionId, path, options, queryId });

export const inspectBackup = (path: string) => invoke<BackupFileInfo>('inspect_backup', { path });

// -- Queries ---------------------------------------------------------------

export const runQuery = (connectionId: string, sqlText: string, queryId: string, limit?: number) =>
  invoke<RunResult>('run_query', { connectionId, sqlText, queryId, limit });

export const cancelQuery = (connectionId: string, queryId: string) =>
  invoke<boolean>('cancel_query', { connectionId, queryId });

export const statementAtCursor = (sqlText: string, offset: number) =>
  invoke<string | null>('statement_at_cursor', { sqlText, offset });

// -- Saved queries and history ---------------------------------------------

export const listSavedQueries = () => invoke<SavedQuery[]>('list_saved_queries');

/** Pass an empty `id` to create; the backend generates one and returns it. */
export const saveQuery = (query: Omit<SavedQuery, 'createdAt' | 'updatedAt'>) =>
  invoke<SavedQuery>('save_query', {
    query: { ...query, createdAt: '', updatedAt: '' },
  });

export const deleteSavedQuery = (id: string) => invoke<void>('delete_saved_query', { id });

export const listHistory = (search?: string, limit?: number) =>
  invoke<HistoryEntry[]>('list_history', { search, limit });

export const clearHistory = () => invoke<void>('clear_history');

export const deleteHistoryEntry = (id: number) => invoke<void>('delete_history_entry', { id });
