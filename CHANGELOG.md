# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **`cargo audit` in CI**: A new scheduled + PR-triggered workflow (`.github/workflows/audit.yml`) runs `cargo audit` against `src-tauri/Cargo.lock`, closing a gap where the last two RUSTSEC advisories (see the 1.1.0 Security section below) had to be found and fixed manually. The weekly schedule catches an advisory published against a dependency already on `main`, which a push/PR-only check never would. Also added `.github/dependabot.yml` for npm, Cargo and GitHub Actions, with `@codemirror/view`/`@codemirror/state` and `rusqlite` excluded (see the comments there for why those three need a deliberate, coordinated bump rather than an automated one).
- **Cancellable Table Export, Backup & Restore**: `export_table`, `backup_database` and `restore_database` now register with the same connection-scoped cancellation registry a query uses, so `cancelQuery(connectionId, queryId)` can stop any of them mid-flight — previously each passed a fresh, untriggerable `CancellationToken::new()` into every `query`/`execute` call, so a "cancel" was structurally impossible no matter what called for it. A cancelled restore rolls back cleanly when it was running inside a transaction. This is a breaking IPC change: the three commands, and the `exportTable`/`backupDatabase`/`restoreDatabase` wrappers in `src/ipc/index.ts`, now require a `queryId` argument. No UI currently calls `cancelQuery` for these three — that's a follow-up, not part of this change.

### Changed
- **Deduplicated Driver Connection Setup**: Extracted the "one connection for user SQL, one for catalog reads" pool setup — previously hand-repeated near-identically in the Postgres, MySQL and SQLite drivers — into one generic `driver::pool::dual_pool` function, made possible because `sqlx::Pool`/`PoolOptions` are already generic over `sqlx::Database`. Also extracted the "one row per column of a composite foreign key, grouped back into one constraint" logic — separately hand-rolled in the SQLite, MySQL and SQL Server drivers — into `driver::fk::group_foreign_keys`, with its own unit tests for the composite-column and out-of-order-id cases the three drivers each had to get right independently.
- **Streamed Table Export**: `export_table` now writes CSV, TSV and SQL exports straight to disk as each page is fetched, instead of accumulating the entire table in memory first (`transfer/export.rs`'s `read_table_paged`, now used only by XLSX/JSON, which genuinely need every row before either format can be written). The paging logic itself — offsets, the truncation flag, the stable ordering that keeps pages from overlapping or skipping rows — moved into one shared `walk_table_pages` helper used by both paths, rather than being duplicated between the accumulating and streaming versions.
- **Toolchain & Dependency Upgrades**: Upgraded frontend toolchain dependencies including Vite 8 (`^8.2.2`), React 19 (`^19.2.8`), TypeScript (`^7.0.2`), Zustand (`^5.0.15`), Tailwind CSS (`^4.3.3`), and Vitest (`^4.1.11`). Upgraded `bson` to `3.1.0` (and switched the `mongodb` crate from its `bson-2` to `bson-3` feature to match), plus `reqwest` (`0.13`), `rust_xlsxwriter` (`0.98.2`), and `calamine` (`0.36`). `rusqlite` stays pinned at `0.39`: `0.40` requires `libsqlite3-sys` `0.38`, which conflicts with the `0.37` that `sqlx-sqlite` links against — the two crates cannot both link the native `sqlite3` library.
- **CI/CD Pipeline Optimizations**: Streamlined GitHub Actions release (`release.yml`) and CI (`ci.yml`) workflows by removing sccache build overhead, disabling incremental Rust compilation for clean builds, and optimizing format check ordering.

### Fixed
- **TypeScript Path Mapping & Test Mock Cleanup**: Configured `@/*` path mapping in `tsconfig.json` for consistent module resolution, and refined mock cleanup handling in Vitest test suites (`ImportDialog.test.tsx`).
- **Dependency & Docs Drift**: Regenerated `Cargo.lock` so it actually satisfies `Cargo.toml` again (`bson` now resolves to the declared `3.1.0`; `rusqlite` reverted from an undeclarable `0.40.2` back to the working `0.39`). Corrected `package.json`'s declared `@codemirror/view` version (`6.43.9` → `6.43.8`) to match what the pnpm override actually installs. Aligned the documented `cargo clippy` command in README/CONTRIBUTING with the one CI actually runs, keeping the `--no-default-features` variant as a documented fast-iteration alternative.
- **MongoDB Browse Filters on Non-String Fields**: Equals/Not Equals/Greater Than/Less Than filters on the table grid now compare against a field's real BSON type (number, boolean, date, ObjectId, ...) instead of always comparing as a string, which previously made these filters silently match nothing on non-string fields.
- **Result Grid Column-Resize Listener Leak**: Dragging a column wider or narrower registered `window` listeners that were only removed on mouse-up; switching tabs or reloading the result mid-drag left them attached for the rest of the session. They are now torn down on unmount too.
- **Result Grid Cell Editor Double-Commit**: Pressing Escape to cancel an in-progress cell edit could, in rare cases, still be followed by a trailing blur event that re-committed the discarded value. The edit is now resolved (committed or cancelled) exactly once.
- **Case-Sensitive Text Filter Fallback**: A grid's Greater Than/Less Than filter fell back to case-sensitive text ordering on non-numeric columns, unlike every other filter operator (Equals, Contains, Starts With), which are case-insensitive.

### Testing
- **Result Grid, Result Panel & Tab Coverage**: Added test suites for `ResultGrid`, `ResultPanel`, `QueryTab`, `TableTab` and `TabBar` — previously the app's largest and most stateful components (virtualization, sort/filter routing, run/cancel guards, out-of-order response handling, dirty-edit tracking) had none. Also fixed `src/test/setup.ts`'s virtualizer size stub, which patched `clientHeight`/`clientWidth` but not the `offsetWidth`/`offsetHeight` properties `@tanstack/virtual-core` actually reads, silently virtualizing every grid down to zero rendered rows in tests.
- **Query, Backup & Restore Cancellation Round-Trip**: Extended `src-tauri/tests/cancellation.rs` (added in the query-cancellation test above) to also cancel a backup mid-table and a restore mid-statement, confirming both actually stop, the cancelled restore rolls back, and the connection is reusable immediately afterward. All three run against an in-memory SQLite database, no Docker fixture required.
- **Streamed Export Correctness**: Added integration tests confirming `export_table_streaming`'s file output is a true partition of the table (no row written twice or skipped) across a page boundary, matching the existing guarantee for the accumulating path — and that an empty table still gets a header row. (The first version of the empty-table test used `nothing` as the table name, which is itself a reserved SQLite keyword — caught once the local `tests/fixtures/faro_test.db` fixture actually existed to run it against.)
- **`group_foreign_keys` Unit Tests**: Covered directly rather than only indirectly through each driver's own integration test: composite-key columns folding into one constraint, and SQLite's out-of-declaration-order ids still producing output in first-seen row order.

## [1.2.0] - 2026-08-19

### Added
- **`useAsyncAction` & `useBackendProgress` Hooks**: Added reusable React hooks for standardized async operation lifecycle management (busy states, error handling, completion tracking) and Tauri progress event subscriptions during background tasks like backups and restores.
- **Import Safety Confirmation**: Added interactive confirmation prompt before writing data in `ImportDialog`, verifying the target table name and row count to prevent accidental overwrites.
- **`FilterInput` UI Component**: Added a centralized `FilterInput` search component (`src/components/ui.tsx`) with customizable styling for consistent list filtering across panels and dialogs.
- **Frontend Feature Unit Tests**: Added Vitest unit test suites for the CodeMirror `Editor` component (`tests/features/editor/Editor.test.tsx`) covering keyboard shortcuts (`Mod-Enter`, `Shift-Enter`), imperative handle methods, and value synchronization, as well as `ImportDialog` (`tests/features/transfer/ImportDialog.test.tsx`).
- **Live Engine Integration Tests**: Expanded Rust integration test suite (`src-tauri/tests/live_engines.rs`) with comprehensive test coverage for ClickHouse, MongoDB, and MSSQL live drivers.

### Changed
- **Async State Refactoring**: Standardized async state management across `BackupDialog`, `ImportDialog`, `ExportDialog`, `ResultGrid`, `ResultPanel`, and `TabBar` using `useAsyncAction`.
- **Updater Toast Entrance Animation**: Replaced uninstalled utility classes with a native `@keyframes toast-in` CSS transition for the updater toast (`UpdaterToast.tsx`).
- **CI/CD & Release Workflows**: Updated GitHub Actions release workflow (`release.yml`) and CI pipeline (`ci.yml`) to upgrade `sccache-action` to `v0.0.11` and refine build caching and artifact packaging.

### Fixed
- **ClickHouse Numeric Validation on Decimal Decoding**: Prevented potential unescaped literal SQL injection in backups/exports by strictly validating (`looks_numeric`) that ClickHouse decimal response strings contain only numeric characters before decoding as decimal literals.
- **MongoDB Read-Only Safeguards & Query Routing**: Overrode `Driver::run` in `MongoDriver` to route Mongo queries directly to read-only document execution, and explicitly blocked mutating pipeline stages (`$out` and `$merge`) in `aggregate`.
- **MSSQL Read-Only Connection Intent**: Enabled `ApplicationIntent=ReadOnly` (`tds.readonly(true)`) on TDS connection setup when read-only mode is configured.
- **Grid Cell Editing Invariant**: Ensured active cell editing state (`editingCell`) is cleared in `ResultGrid` whenever a new result set is loaded.

## [1.1.0] - 2026-08-12

### Added
- **SSL/TLS Connection Configurations**: Added support for custom CA certificates, client certificates, client keys, and SSL connection modes (disable, require, verify-ca, verify-full) across PostgreSQL, MySQL, MariaDB, and MSSQL.
- **Command Palette**: Added an interactive keyboard-first command palette (`Cmd+K` / `Ctrl+K`) for quick actions, view navigation, and tab switching.
- **Frontend Component & Store Unit Tests**: Added Vitest unit test suites for UI components, Zustand state stores, and feature dialogs.

### Changed
- **App Icon Relocation**: Migrated application icon to `resources/faro.png` for better project structure.
- **SQLite Foreign Key Introspection**: Refactored schema reflection to correctly group multi-column compound foreign keys.
- **Test Suite Restructuring**: Relocated frontend unit tests (`*.test.ts`, `*.test.tsx`) into the root `tests/` directory and updated Vitest and TypeScript compilation configurations.
- **Docker Compose Test File Relocation**: Moved `docker-compose.test.yml` into the root `tests/` directory to consolidate test assets, updating test scripts and CI workflows.

### Fixed
- **Decimal Precision & Normalization on PostgreSQL and MySQL**: `numeric` and `DECIMAL` values are now decoded through `BigDecimal` instead of `rust_decimal` (resolving its ~28-significant-digit limitation) and normalized to strip trailing fractional zeros without losing numeric precision.
- **Text Filters on ClickHouse**: Browse filters using "contains" and "starts with" no longer emit a `LIKE ... ESCAPE` clause on ClickHouse, which rejects it as a syntax error. Wildcards are still escaped — ClickHouse treats backslash as the escape character by default.
- **CI Workflows & Secret Storage Isolation**: Updated test database file references in GitHub Actions workflows (`ci.yml` and `release.yml`) to point to `tests/docker-compose.test.yml`, and randomized key values in secret storage integration tests to avoid key collisions.

### Security
- **Tiberius TLS Migration & Security Vulnerability Resolution**: Migrated the `tiberius` MSSQL driver from `rustls-tls` to `native-tls` and updated transitive Cargo dependencies (`rustls-webpki`, `glib`, `rand`) to eliminate security vulnerabilities (RUSTSEC-2025-0003, RUSTSEC-2025-0008, and Dependabot security alerts).
- **CSV Formula Injection Sanitization**: Implemented automatic formula escaping (`'`, `=`, `+`, `-`, `@`, `\t`, `\r`) during CSV exports to prevent spreadsheet macro execution vulnerabilities.
- **Enhanced Read-Only Validation**: Expanded the SQL parser and query validator to strictly detect and block data-modifying statements and DDL operations in read-only connection mode.

## [1.0.0] - 2026-08-09

### Added

- First implementation of Faro.