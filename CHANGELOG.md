# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Updated frontend and Rust dependencies, including the SQL editor, virtualized
  grid, DuckDB and Excel import/export libraries.
- CI now cancels superseded runs and reports database startup diagnostics when
  integration test services fail to start.

### Fixed

- Pinned `rusqlite` to a version that shares the native SQLite library with
  SQLx, avoiding conflicting SQLite library dependencies during builds.
- Pinned the SQL Server driver to the version used by its connection pool.

### Security

- Disabled unused Windows authentication dependencies in the SQL Server
  connection pool, removing the vulnerable `rand` 0.7 dependency. SQL Server
  connections continue to use username and password authentication.

## [1.1.0] - 2026-10-16

### Added

- Query tabs now survive a restart: an unsaved query's title, connection, text
  and row limit are restored on the next launch. Table tabs and results are not
  restored, being reopened or re-run in a click.
- Exports of a query's or a table page's results are written from the backend's
  own copy of the rows rather than from the UI, so values the grid only shows as
  a size (large binary data) export faithfully.
- Whole-table JSON exports now stream to disk page by page, like CSV and TSV,
  instead of holding the entire table in memory twice.
- A confirmation before ⌘↵ runs a multi-statement script that writes data or
  schema, which points to ⌘⇧↵ for running only the statement under the cursor.
- Editing a table through a binary primary key now works: keys are sent and
  matched as the value they were read as, not the text the grid displays.
- Per-engine SQL highlighting and quoting in the editor for MySQL, MariaDB and
  SQL Server (backtick and bracket identifiers), alongside Postgres and SQLite.
- SQL Server backups preserve `IDENTITY` columns and restore their values,
  resetting the identity past the highest key inserted.
- Postgres backups rebuild a faithful `CREATE TABLE`, keeping identity and
  generated columns, sequences, `UNIQUE`, `CHECK` and `EXCLUDE` constraints,
  enum types and array columns.
- DuckDB support for nested types (lists, arrays, `STRUCT`, `MAP`, `UNION`) and
  for intervals and temporal values, and working statement cancellation.
- CSV imports detect the field separator from the file, so the semicolon-
  separated files Excel writes in much of Europe import as columns.
- Failed statements now carry the engine's own error code (SQLSTATE or SQL
  Server number), shown beneath the message.
- A disconnect action on a connection is now distinct from deleting it, and
  asks first when it would cancel a running query or strand unsaved changes.
- SQL Server stored procedures are run as queries, so `EXEC` shows its result
  set rather than only a row count.

### Changed

- ⇧↵ no longer runs a query; ⌘↵ is the run shortcut, as Shift-Enter is a newline
  in most other editors.
- "Run statement under cursor" resolves the statement against the editor's live
  document and cursor rather than a debounced hint, so it can no longer run the
  statement the cursor has already left.
- Table tabs remember their page, sort and filters across switches, and show the
  page already loaded instead of re-querying; a superseded load is cancelled.
- Backups and exports are written to a temporary file and moved into place only
  when complete, so a failed or cancelled one leaves no file that looks finished.
- Backups drop tables children-first, and skip redundant foreign keys where the
  engine's own `CREATE TABLE` already declares them.
- Postgres user queries run over the simple protocol, so enums, `inet`, ranges,
  arrays, `interval` and extension types are read instead of showing as
  unsupported.
- Binary values larger than 256 bytes cross to the UI as their size alone, and
  integers beyond JavaScript's safe range cross as exact decimal text rather
  than being silently rounded.
- The pool never retires the session connection, so an open transaction, temp
  tables and session settings survive a long idle period.
- MySQL's read-only session setting is applied to every connection the pool
  opens, not once at connect, so it cannot be lost when a connection is replaced.
- Listing collections and inferring their fields on MongoDB runs concurrently
  rather than one round trip at a time.
- Keychain calls run off the main thread, so a slow or prompting keychain no
  longer freezes the window.
- Quitting waits at most a few seconds for connections to close rather than
  blocking on an unreachable server.

### Fixed

- Statements led by a comment are classified correctly, so a commented `SELECT`
  is no longer run as a write or refused on a read-only connection.
- A cursor resting just after a statement's `;` on the same line now runs that
  statement, not the one on the next line.
- Comment-only fragments are dropped before execution, so a script ending in a
  comment no longer fails on MySQL with "Query was empty".
- The read-only guard now refuses `PRAGMA name(value)` writes and side-effecting
  functions such as `set_config`, `nextval`, `dblink` and the `lo_*` family that
  have no write keyword.
- SQL Server backups write booleans as `bit` values (`1`/`0`) instead of
  `TRUE`/`FALSE`, which failed to restore.
- Postgres arrays are dumped as `'{…}'` text, so typed arrays restore.
- Spreadsheet-formula escaping no longer prefixes numbers, dates or ids, which
  turned every negative number into text.
- Values Faro cannot read are refused on export and backup with an explanation,
  rather than being written out as the column's type name.
- Excel exports refuse a table larger than a worksheet up front, before writing.
- Grid edits no longer treat `interval`, `point` or array columns as integers
  and emit an unquoted number where the engine needs quoted text.
- A paginated query ending in a line comment no longer swallows the closing
  parenthesis and fails to parse.

### Removed

- The `export_result` command, replaced by `export_cached`.

### Security

- Read-only connections are enforced beyond the leading keyword: a write can no
  longer hide behind a `PRAGMA` call or a side-effecting function, and MySQL's
  session read-only setting cannot be lost when the pool replaces a connection.
- MongoDB connections that would not negotiate TLS now say so, since credentials
  would otherwise cross the network in the clear.

## [1.0.0] - 2026-09-24

### Added

- First implementation of Faro.
