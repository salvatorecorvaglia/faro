# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.4.0] - 2026-08-29

### Added
- **Batched Delimited File Import**: `read_rows_batched` (`src-tauri/src/transfer/import.rs`) reads CSV and TSV files in bounded streaming chunks (e.g. 5,000 rows per batch) and inserts them within managed transactions, replacing whole-file in-memory materialization that previously caused memory exhaustion on large datasets. Also added cancellation support for import operations via `cancellation::register` / `check_cancelled`.
- **Query Tab Row Limit Selector**: Added a configurable row limit selector (`1,000`, `10,000`, `50,000`, `100,000`) to query tabs (`src/features/tabs/QueryTab.tsx`). The limit is persisted in the tab store across tab switches and clamped by the backend's `MAX_PAGE` limit.
- **Dynamic OS Keyring Re-probing & Dual-Store Lookup**: `keychain_available` (`src-tauri/src/secrets.rs`) now dynamically re-probes for OS keychain availability with an atomic check throttled to a 5-second backoff, allowing keyrings unlocked after application startup (such as Linux login keyrings) to be seamlessly used without restarting Faro. In addition, `get_password` and `delete_password` now query and clear both the OS keychain and in-memory fallback stores to prevent orphaned credentials during keyring state transitions.
- **ClickHouse Protocol Probing (`SslMode::Prefer`)**: `ClickHouseDriver` (`src-tauri/src/driver/clickhouse.rs`) now supports `SslMode::Prefer` (probing `https` and falling back to `http`), skips certificate verification on `Require` and `Prefer` SSL modes (encrypt without authenticating) while verifying trust store chains on `VerifyCa` and `VerifyFull`, and executes a proactive ping on connect to validate host, scheme, and credentials in a single round-trip.

### Changed
- **Modular SQLite Store Architecture**: Decomposed the monolithic backend store (`src-tauri/src/store.rs`) into modular components under `src-tauri/src/store/`: `connections.rs` (connection metadata, passwords, validation), `library.rs` (saved queries and execution history), and `mod.rs` (schema migration, store initialization, and shared utilities).
- **Frontend Component Decomposition**: Extracted `SchemaTree.tsx` from `Sidebar.tsx` for cleaner hierarchy and maintainability, and split `ResultGrid.tsx` into dedicated `GridCell.tsx` (for cell rendering and clipboard copying) and `gridLayout.ts` (for virtualized column width and layout calculations).
- **Toolchain & Dependency Upgrades**: Bumped `@types/node` (`^26.4.0`), `uuid` (`1.26.0`), and updated updater toast messaging (`src/components/UpdaterToast.tsx`).

### Fixed
- **Unsaved Edits Protection on Tab Navigation**: `openQueryTab` and `openTableTab` in `useTabs` (`src/state/tabs.ts`) now route focus changes through `focusAfterGuard`, prompting the user before discarding unsaved staged edits when opening tabs from the sidebar schema tree or keyboard shortcuts.
- **Debounced Filter Rollback**: When editing table filters in `TableTab` (`src/features/tabs/TableTab.tsx`), declining the confirmation prompt to discard unsaved staged edits now rolls back the filter inputs to the currently displayed filter set, preventing UI filter inputs and displayed grid data from becoming desynchronized.
- **Hanging Promises on Displaced Confirmation Dialogs**: `confirmDialog` (`src/state/confirm.ts`) now cleanly resolves any displaced pending confirmation promise with `false`, preventing chained async operations from stalling indefinitely when a new dialog request arrives.
- **SQL History Search Wildcard Escaping**: Added `escape_like` in `src-tauri/src/store/mod.rs` so searching execution history with `Store::list_history` properly escapes `%`, `_`, and `\` characters rather than interpreting them as SQL wildcards.

### Testing
- **Frontend Lifecycle & State Coverage**: Added unit test suites for root `App` (`tests/App.test.tsx`), `useAsyncAction` and `useBackendProgress` hooks (`tests/hooks/useAsyncAction.test.ts`), `confirmDialog` displacement and resolution (`tests/state/confirm.test.ts`), `useTabs` dirty guard navigation (`tests/state/tabs.test.ts`), and query toolbar row limits (`tests/features/tabs/QueryTab.test.tsx`).
- **Batched Import & Keyring Fallback Verification**: Added Rust integration tests covering bounded batch chunking and format parity in `read_rows_batched` (`src-tauri/src/transfer/import.rs`), as well as session-fallback keyring recovery and cleanup (`src-tauri/src/secrets.rs`).

## [1.3.0] - 2026-08-25

### Added
- **`cargo audit` in CI**: A new scheduled + PR-triggered workflow (`.github/workflows/audit.yml`) runs `cargo audit` against `src-tauri/Cargo.lock`, closing a gap where the last two RUSTSEC advisories (see the 1.1.0 Security section below) had to be found and fixed manually. The weekly schedule catches an advisory published against a dependency already on `main`, which a push/PR-only check never would. Also added `.github/dependabot.yml` for npm, Cargo and GitHub Actions, with `@codemirror/view`/`@codemirror/state` and `rusqlite` excluded (see the comments there for why those three need a deliberate, coordinated bump rather than an automated one).
- **Cancellable Table Export, Backup & Restore**: `export_table`, `backup_database` and `restore_database` now register with the same connection-scoped cancellation registry a query uses, so `cancelQuery(connectionId, queryId)` can stop any of them mid-flight — previously each passed a fresh, untriggerable `CancellationToken::new()` into every `query`/`execute` call, so a "cancel" was structurally impossible no matter what called for it. A cancelled restore rolls back cleanly when it was running inside a transaction. This is a breaking IPC change: the three commands, and the `exportTable`/`backupDatabase`/`restoreDatabase` wrappers in `src/ipc/index.ts`, now require a `queryId` argument. No UI currently calls `cancelQuery` for these three — that's a follow-up, not part of this change.

### Changed
- **Deduplicated Driver Connection Setup**: Extracted the "one connection for user SQL, one for catalog reads" pool setup — previously hand-repeated near-identically in the Postgres, MySQL and SQLite drivers — into one generic `driver::pool::dual_pool` function, made possible because `sqlx::Pool`/`PoolOptions` are already generic over `sqlx::Database`. Also extracted the "one row per column of a composite foreign key, grouped back into one constraint" logic — separately hand-rolled in the SQLite, MySQL and SQL Server drivers — into `driver::fk::group_foreign_keys`, with its own unit tests for the composite-column and out-of-order-id cases the three drivers each had to get right independently.
- **Streamed Table Export**: `export_table` now writes CSV, TSV and SQL exports straight to disk as each page is fetched, instead of accumulating the entire table in memory first (`transfer/export.rs`'s `read_table_paged`, now used only by XLSX/JSON, which genuinely need every row before either format can be written). The paging logic itself — offsets, the truncation flag, the stable ordering that keeps pages from overlapping or skipping rows — moved into one shared `walk_table_pages` helper used by both paths, rather than being duplicated between the accumulating and streaming versions.
- **Toolchain & Dependency Upgrades**: Upgraded frontend toolchain dependencies including Vite 8 (`^8.2.2`), React 19 (`^19.2.8`), TypeScript (`^7.0.2`), Zustand (`^5.0.15`), Tailwind CSS (`^4.3.3`), and Vitest (`^4.1.11`). Upgraded `bson` to `3.1.0` (and switched the `mongodb` crate from its `bson-2` to `bson-3` feature to match), plus `reqwest` (`0.13`), `rust_xlsxwriter` (`0.98.2`), and `calamine` (`0.36`). `rusqlite` stays pinned at `0.39`: `0.40` requires `libsqlite3-sys` `0.38`, which conflicts with the `0.37` that `sqlx-sqlite` links against — the two crates cannot both link the native `sqlite3` library.
- **CI/CD Pipeline Optimizations**: Streamlined GitHub Actions release (`release.yml`) and CI (`ci.yml`) workflows by removing sccache build overhead, disabling incremental Rust compilation for clean builds, and optimizing format check ordering.
- **Destructive Confirmations Use the App's Own Dialog**: Every place that asked "are you sure?" before an irreversible action — closing or switching away from a dirty tab, deleting a saved query or a connection, clearing history, applying staged edits, importing a file, restoring a backup — used the browser's own unstyled, event-loop-blocking `confirm()`, visually inconsistent with every other dialog in the app. All eight call sites now go through a new `confirmDialog()` (`src/state/confirm.ts`) backed by the same `Modal` component everything else uses, rendered once via `<ConfirmHost />` near the app root. Usable from anywhere a `confirm()` call was, including the tabs store's `closeTab`/`setActive` — which have no component of their own to hold dialog state — since it is just a promise, not a hook. Those two store actions are now `async` as a result; every call site was already fire-and-forget.

### Fixed
- **TypeScript Path Mapping & Test Mock Cleanup**: Configured `@/*` path mapping in `tsconfig.json` for consistent module resolution, and refined mock cleanup handling in Vitest test suites (`ImportDialog.test.tsx`).
- **Dependency & Docs Drift**: Regenerated `Cargo.lock` so it actually satisfies `Cargo.toml` again (`bson` now resolves to the declared `3.1.0`; `rusqlite` reverted from an undeclarable `0.40.2` back to the working `0.39`). Corrected `package.json`'s declared `@codemirror/view` version (`6.43.9` → `6.43.8`) to match what the pnpm override actually installs. Aligned the documented `cargo clippy` command in README/CONTRIBUTING with the one CI actually runs, keeping the `--no-default-features` variant as a documented fast-iteration alternative.
- **MongoDB Browse Filters on Non-String Fields**: Equals/Not Equals/Greater Than/Less Than filters on the table grid now compare against a field's real BSON type (number, boolean, date, ObjectId, ...) instead of always comparing as a string, which previously made these filters silently match nothing on non-string fields.
- **Result Grid Column-Resize Listener Leak**: Dragging a column wider or narrower registered `window` listeners that were only removed on mouse-up; switching tabs or reloading the result mid-drag left them attached for the rest of the session. They are now torn down on unmount too.
- **Result Grid Cell Editor Double-Commit**: Pressing Escape to cancel an in-progress cell edit could, in rare cases, still be followed by a trailing blur event that re-committed the discarded value. The edit is now resolved (committed or cancelled) exactly once.
- **Case-Sensitive Text Filter Fallback**: A grid's Greater Than/Less Than filter fell back to case-sensitive text ordering on non-numeric columns, unlike every other filter operator (Equals, Contains, Starts With), which are case-insensitive.
- **Schema Tree Table Rows Were Not Keyboard-Reachable**: Table rows in the sidebar's schema tree were plain `onClick` `<div>`s with no `role`, `tabIndex` or key handler — unlike the schema-toggle and connection rows immediately above them in the same tree, which already used the shared `rowActivation` helper. They now match.
- **Updater Logging Shipped to Every User's Console**: `downloadAndInstall` and `restartApp` logged unconditionally in both development and production; only the decision to *check* for updates was gated to production builds, not the logging inside the functions that run once an update is found. All updater logging is now development-only.
- **`CONTRIBUTING.md` Claimed the Wrong `rusqlite` Version**: Still said `rusqlite` 0.40 after the dependency work above reverted it to 0.39 — corrected, with a pointer to the `Cargo.toml` comment explaining why 0.40 doesn't work here.

### Documentation
- **SQL Server Read-Only Is Weaker on a Standalone Server**: Added an explicit README note (in the engine table) and a `CONTRIBUTING.md` pointer for contributors touching read-only enforcement: a standalone (non–Always On) SQL Server ignores the `ApplicationIntent=ReadOnly` flag Faro sets and accepts writes regardless, unlike every other supported engine, which also rejects writes at the server. Faro's own SQL statement validator was already the primary guard in every case; this makes explicit that for a standalone SQL Server, it is the *only* one — previously this was known only from the driver's internal comments, not disclosed anywhere a user or contributor would see it before relying on it.

### Testing
- **Result Grid, Result Panel & Tab Coverage**: Added test suites for `ResultGrid`, `ResultPanel`, `QueryTab`, `TableTab` and `TabBar` — previously the app's largest and most stateful components (virtualization, sort/filter routing, run/cancel guards, out-of-order response handling, dirty-edit tracking) had none. Also fixed `src/test/setup.ts`'s virtualizer size stub, which patched `clientHeight`/`clientWidth` but not the `offsetWidth`/`offsetHeight` properties `@tanstack/virtual-core` actually reads, silently virtualizing every grid down to zero rendered rows in tests.
- **Query, Backup & Restore Cancellation Round-Trip**: Extended `src-tauri/tests/cancellation.rs` (added in the query-cancellation test above) to also cancel a backup mid-table and a restore mid-statement, confirming both actually stop, the cancelled restore rolls back, and the connection is reusable immediately afterward. All three run against an in-memory SQLite database, no Docker fixture required.
- **Streamed Export Correctness**: Added integration tests confirming `export_table_streaming`'s file output is a true partition of the table (no row written twice or skipped) across a page boundary, matching the existing guarantee for the accumulating path — and that an empty table still gets a header row. (The first version of the empty-table test used `nothing` as the table name, which is itself a reserved SQLite keyword — caught once the local `tests/fixtures/faro_test.db` fixture actually existed to run it against.)
- **`group_foreign_keys` Unit Tests**: Covered directly rather than only indirectly through each driver's own integration test: composite-key columns folding into one constraint, and SQLite's out-of-declaration-order ids still producing output in first-seen row order.
- **Confirm Dialog & Sidebar Coverage**: Added tests for `confirmDialog`/`ConfirmHost` (resolves true/false via each button and via Escape, honours a custom label), and a first test file for the sidebar's schema tree, covering the keyboard-reachability fix above. Writing the latter surfaced two pre-existing, unrelated latent bugs worth a follow-up: `LibraryPanel` and `ConnectionDialog` both crash outright (reading `.length`/calling `.find` on `undefined`) if their respective backend calls ever resolve to `undefined` instead of an empty array, rather than defaulting defensively.

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