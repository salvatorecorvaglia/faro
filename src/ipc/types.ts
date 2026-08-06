/**
 * Mirrors the Rust types in `src-tauri/src/model`.
 *
 * These must stay in sync with the serde representations: the Rust side uses
 * `rename_all = "camelCase"`, and `Value` is an internally-tagged enum with
 * `tag = "kind"` / `content = "value"`.
 */

export type Engine =
  | 'postgres'
  | 'mysql'
  | 'mariadb'
  | 'sqlite'
  | 'sqlserver'
  | 'duckdb'
  | 'clickhouse'
  | 'cockroachdb'
  | 'redshift'
  | 'mongodb';

export type SslMode = 'disable' | 'prefer' | 'require';

export interface ConnectionConfig {
  id: string;
  name: string;
  engine: Engine;
  host: string;
  port: number;
  username: string;
  database: string;
  filePath: string | null;
  sslMode: SslMode;
  color: string | null;
  readOnly: boolean;
}

export interface ConnectionStatus extends ConnectionConfig {
  connected: boolean;
  hasPassword: boolean;
}

export interface EngineInfo {
  id: Engine;
  name: string;
  defaultPort: number;
  fileBased: boolean;
  supported: boolean;
  /** False for MongoDB, whose queries are documents rather than SQL. */
  isSql: boolean;
}

/** A single cell. `Null` carries no `value` field at all. */
export type Value =
  | { kind: 'null' }
  | { kind: 'bool'; value: boolean }
  | { kind: 'int'; value: number }
  | { kind: 'float'; value: number }
  | { kind: 'decimal'; value: string }
  | { kind: 'text'; value: string }
  | { kind: 'bytes'; value: number[] }
  | { kind: 'date'; value: string }
  | { kind: 'time'; value: string }
  | { kind: 'timestamp'; value: string }
  | { kind: 'uuid'; value: string }
  | { kind: 'json'; value: unknown }
  | { kind: 'array'; value: Value[] }
  | { kind: 'unsupported'; value: string };

export interface ColumnInfo {
  name: string;
  typeName: string;
}

export interface ResultSet {
  columns: ColumnInfo[];
  rows: Value[][];
  truncated: boolean;
  elapsedMs: number;
}

export interface ExecResult {
  rowsAffected: number;
  elapsedMs: number;
}

export type QueryOutcome = ({ type: 'rows' } & ResultSet) | ({ type: 'affected' } & ExecResult);

export interface StatementResult {
  sql: string;
  outcome: QueryOutcome | null;
  error: string | null;
}

export interface RunResult {
  statements: StatementResult[];
  totalElapsedMs: number;
}

export type TableKind = 'table' | 'view' | 'materializedView';

export interface TableRef {
  schema: string | null;
  name: string;
}

export interface SchemaInfo {
  name: string;
  isSystem: boolean;
}

export interface TableInfo {
  schema: string | null;
  name: string;
  kind: TableKind;
  estimatedRows: number | null;
}

export interface TableColumns {
  schema: string | null;
  name: string;
  columns: string[];
}

export interface ColumnDetail {
  name: string;
  typeName: string;
  nullable: boolean;
  default: string | null;
  isPrimaryKey: boolean;
  ordinal: number;
}

export interface ForeignKey {
  name: string;
  columns: string[];
  referencedTable: TableRef;
  referencedColumns: string[];
}

export interface IndexInfo {
  name: string;
  columns: string[];
  isUnique: boolean;
  isPrimary: boolean;
}

export interface TableDetail {
  table: TableRef;
  kind: TableKind;
  columns: ColumnDetail[];
  primaryKey: string[];
  foreignKeys: ForeignKey[];
  indexes: IndexInfo[];
}

export type FilterOp =
  | 'equals'
  | 'notEquals'
  | 'contains'
  | 'startsWith'
  | 'greaterThan'
  | 'lessThan'
  | 'isNull'
  | 'isNotNull';

export interface ColumnFilter {
  column: string;
  op: FilterOp;
  value: string;
}

export interface BrowseOptions {
  sortColumn?: string | null;
  sortDesc?: boolean;
  filters?: ColumnFilter[];
  limit?: number | null;
  offset?: number;
}

export interface SavedQuery {
  id: string;
  name: string;
  folder: string | null;
  sql: string;
  connectionId: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface HistoryEntry {
  id: number;
  sql: string;
  connectionId: string | null;
  /** Copied at write time, so it survives deleting the connection. */
  connectionName: string | null;
  executedAt: string;
  durationMs: number;
  rowCount: number;
  error: string | null;
  succeeded: boolean;
}

/**
 * A value staged in the grid.
 *
 * `null` and `text` are distinct because an emptied cell is otherwise
 * ambiguous; `default` omits the column so the database supplies its own value.
 */
export type EditValue = { kind: 'null' } | { kind: 'text'; value: string } | { kind: 'default' };

export interface CellEdit {
  column: string;
  value: EditValue;
}

export type PendingChange =
  | { type: 'update'; key: CellEdit[]; cells: CellEdit[] }
  | { type: 'insert'; cells: CellEdit[] }
  | { type: 'delete'; key: CellEdit[] };

export interface GuardedStatement {
  sql: string;
  /** Rows this statement must affect, or null when unchecked. */
  expect: number | null;
}

export interface ApplyResult {
  statementsRun: number;
  rowsAffected: number;
}

export type ExportFormat = 'csv' | 'tsv' | 'json' | 'sql' | 'xlsx';

export interface ExportOptions {
  format: ExportFormat;
  includeHeader: boolean;
  tableName?: string | null;
}

export type ImportFormat = 'csv' | 'tsv' | 'json' | 'xlsx';

export interface ImportPreview {
  columns: string[];
  sampleRows: string[][];
  totalRows: number | null;
  inferredTypes: string[];
}

export interface ColumnMapping {
  sourceIndex: number;
  targetColumn: string;
}

export interface ImportOptions {
  format: ImportFormat;
  hasHeader: boolean;
  mappings: ColumnMapping[];
  nullTokens: string[];
}

export interface TransferResult {
  rows: number;
  path: string;
}

export interface BackupOptions {
  tables: TableRef[];
  includeSchema: boolean;
  includeData: boolean;
  dropExisting: boolean;
}

export interface BackupResult {
  tables: number;
  rows: number;
  bytes: number;
  path: string;
}

export interface BackupProgress {
  table: string;
  tableIndex: number;
  tableCount: number;
  rowsWritten: number;
}

export interface RestoreOptions {
  stopOnError: boolean;
}

export interface RestoreResult {
  statements: number;
  failed: number;
  errors: string[];
}

export interface BackupFileInfo {
  statements: number;
  bytes: number;
  hasSchema: boolean;
  firstLines: string[];
}

/** The shape `FaroError` serializes to. */
export interface FaroError {
  kind:
    | 'connection'
    | 'database'
    | 'notConnected'
    | 'cancelled'
    | 'unsupportedEngine'
    | 'store'
    | 'keychain'
    | 'sql'
    | 'io'
    | 'other';
  message: string;
  code?: string;
}
