# Audit completion and validation

All implementation phases remain uncommitted. This file supersedes the earlier list of unverified Phase 5 checks.

## Completed implementation

- Query execution and cursor lookups live in `useQueryExecution`; table metadata and paging live in `useTableBrowse`. Table writes and transfers share `useAsyncAction` state and synchronous re-entry protection.
- Script inspection and deferred formatting refuse to apply to a document or connection changed while awaiting a response. Sorting cancels an older filter debounce.
- Rust and frontend tests consume `ipc/fixtures/contract.json`, covering exact large integers, inline and omitted binary data, nested values, optimistic edit snapshots and structured errors.
- SQL formatting and tab UI load on demand. `pnpm check:bundle` enforces a 500 kB maximum for each chunk and initial JavaScript including preloads, and refuses native test code in production assets.
- Pages over 2,000 rows sort and filter in a worker. The worker receives a page once and returns row indices; stale replies cannot replace a newer request. Workers terminate on unmount or when no longer needed. Smaller pages retain the fast synchronous path.
- Wide schemas virtualize columns as well as rows. Keyboard navigation scrolls the selected column into view and follows displayed column order. Filter inputs retain their own navigation keys.
- Modal content scrolls within the viewport. Native browser focus handling is retained, with explicit opener restoration for WebKit mouse clicks. Shortcut labels are platform aware and tab close controls appear on keyboard focus.
- Contrast tests calculate at least 4.5:1 for secondary text, NULL indicators and SQL comments across normal, inset, alternating and selected surfaces in both themes.
- MySQL/MariaDB snapshots require InnoDB throughout the connected database; SQL Server snapshots require the server's existing opt-in. Faro does not alter server settings. SQL Server isolation returns to READ COMMITTED after cleanup. Unavailable snapshots require explicit best-effort opt-in.
- MySQL transaction control uses the simple protocol; prepared execution cannot handle every transaction statement. Restore cancellation is checked again immediately before commit.

## Repeating the checks

```sh
pnpm lint
pnpm typecheck
pnpm test:coverage
pnpm build
pnpm check:bundle
pnpm exec playwright install chromium webkit
pnpm test:browser
pnpm test:browser:startup
pnpm test:desktop
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets --no-deps -- -D warnings
FARO_REQUIRE_LIVE=1 cargo test --manifest-path src-tauri/Cargo.toml --locked
```

The native harness builds separate optimized frontend assets in a temporary directory, compiles a debug Tauri binary with `desktop-smoke,custom-protocol`, and uses incognito webviews plus disposable SQLite/settings files. Smoke commands are compiled only in explicit debug smoke builds. It tests real native IPC, snapshot backup, large result sorting, staged edits, close/quit decline and acceptance, import/restore running-operation warnings and cancellation. It simulates an update-ready state and clicks the real Restart now action, declining its confirmation without installing an update or restarting the user's application. On macOS it additionally creates, locks, unlocks and deletes a private keychain with prompts disabled only in that test process. Native artifacts and reports stay in the printed temporary directory.

The browser fixtures use the real UI components. The production startup test mocks only Tauri IPC. Layout, dialog behavior, keyboard handling, worker execution and virtualization use real Chromium and WebKit engines. Screenshots and failure traces are local artifacts; timings have no flaky unit-test thresholds.

## Measurements

Recorded on this machine with Node v24.18.0. These are local observations, not release performance guarantees.

| Measurement                                       | Original baseline | Completed frontend |
| ------------------------------------------------- | ----------------: | -----------------: |
| Initial entry JavaScript, Vite minified           |       1,068.60 kB |          345.46 kB |
| Initial entry JavaScript, Vite gzip               |         331.41 kB |          107.34 kB |
| Initial JavaScript including module preloads      |                 — |          346.19 kB |
| 50,000-row natural text sort, Node median of five |       1,149.83 ms |           95.86 ms |

Production browser observations: Chromium shell 69.5 ms, editor 417.6 ms; WebKit shell 114 ms, editor 543 ms, measured from navigation start on localhost. Browser worker-sort checks processed 30 frames over 626 ms in Chromium and 34 frames over 786 ms in WebKit while sorting twice, rendering, and running test assertions. The grid rendered 156 cells for the 50,000-row fixture. A separate 1,000-column fixture rendered fewer than 30 headers and fewer than 2,000 cells, and keyboard navigation reached both ends.

The formatter is a separate 262.31 kB chunk. No chunk exceeds Vite's 500 kB warning threshold. `node scripts/benchmark-grid.mjs` repeats the pure Node measurement. Worker page cloning and rendered-row mapping still have costs; virtualization and the retained-data budgets do not promise constant memory for arbitrary inputs.

## Live database evidence and platform scope

The full mandatory local suite passed against disposable PostgreSQL 17, MySQL 8.4, MariaDB 11, SQL Server 2022, ClickHouse 24 and MongoDB 8 containers, with no skips. It includes snapshot comparisons after concurrent commits, refusal to guarantee snapshots for MyISAM, SQL Server's opt-in and isolation cleanup, and cancellation of managed writes blocked by real row locks. On this Mac, SQL Server tests used `FARO_TEST_MSSQL_NO_TLS=1` for the disposable loopback fixture; this does not change production TLS defaults.

CI now runs browser checks, production startup and bundle budgets, plus native smoke and file-publication tests on macOS, Windows and Linux (Xvfb). Windows/Linux native jobs have been configured but cannot be reported as executed from this macOS workstation. No commits or remote CI runs were created. OS-specific credential prompts, application menu clicks and release-signed installer behavior are broader release checks than the automated bridge tests; the harness does not claim to emulate those interactions.

Snapshot semantics follow the engine's guarantees and do not protect against every concurrent schema change. References: [MySQL consistent snapshots](https://dev.mysql.com/doc/refman/8.0/en/commit.html), [SQL Server snapshot isolation](https://learn.microsoft.com/en-us/sql/connect/ado-net/sql/snapshot-isolation-sql-server).

## Requested dependency migration

The newer dependency versions are preserved. SQLx 0.9 engine crates are linked directly for PostgreSQL/MySQL to avoid its facade resolving an incompatible optional native SQLite library. Local SQLite sessions now use rusqlite 0.42, with separate editor/catalog/managed connections on blocking threads, bounded result collection and native interruption. Cancellation cannot interrupt a different session owner, rollback or a commit already issued. SQL Server uses a small bb8 manager built against Tiberius 0.13, including routed connections and fully drained validation queries. The declared Rust minimum is 1.94, matching SQLx 0.9's actual requirement. The full suite was repeated after this migration: 504 Rust tests passed, including all 35 mandatory live-engine tests. The native custom-protocol smoke checks passed with the upgraded drivers, including the ready-update UI and both close/quit scenarios.

## Dependency security scan

`pnpm audit` reports no known vulnerabilities in the current frontend lockfile. `cargo audit --file src-tauri/Cargo.lock` completes with no vulnerability errors and five warnings in transitive dependencies; these have not been hidden or waived:

| Package                | Warning                                        | Advisory                                                              |
| ---------------------- | ---------------------------------------------- | --------------------------------------------------------------------- |
| proc-macro-error 1.0.4 | Unmaintained                                   | [RUSTSEC-2024-0370](https://rustsec.org/advisories/RUSTSEC-2024-0370) |
| zbus_names 4.3.5       | Unmaintained                                   | [RUSTSEC-2026-0371](https://rustsec.org/advisories/RUSTSEC-2026-0371) |
| zvariant 5.15.1        | Unmaintained                                   | [RUSTSEC-2026-0370](https://rustsec.org/advisories/RUSTSEC-2026-0370) |
| zvariant_derive 5.15.1 | Unmaintained                                   | [RUSTSEC-2026-0372](https://rustsec.org/advisories/RUSTSEC-2026-0372) |
| glib 0.18.5            | Unsound VariantStrIter iterator implementation | [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429) |

These come through platform/framework integrations. A passing Cargo audit exit code does not mean the warnings are resolved. In particular, the GTK/GLib warning remains relevant to Linux builds and requires compatible upstream framework changes; no incompatible override or downgrade was applied to conceal it.

## Final recorded result

- Frontend: 401 tests passed; coverage 83.4% statements, 75.56% branches, 76.23% functions, 85.44% lines. The focused backup suite also passed after resetting best-effort consent on each opening.
- Backend: 504 tests passed with default features and `FARO_REQUIRE_LIVE=1`; 347 unit tests plus 157 integration tests, including 35 live-engine cases, with no skips.
- Native: both close and quit scenarios exited successfully after all checks. This includes an actual ready-update Restart now action declined with edits preserved, real import/restore cancellation, snapshot backup, and the 50,000-row worker sort. Separate native startup observations were 2,721 ms and 2,935 ms to the frontend bridge in the bundled debug smoke app; these are not release-binary measurements.
- Browser: six UI/grid checks and two production startup checks passed in Chromium and WebKit. Modal screenshots were visually inspected at the minimum window size.
- Gates: lint, typecheck, production build, bundle budget, Rust formatting, all-target Clippy with warnings denied, and version consistency passed.
- Test infrastructure: disposable `faro-audit` containers and test volumes were removed after validation. Local logs and native reports remain under `/tmp/faro-*` and the temporary native report directory printed by `pnpm test:desktop`.

Windows/Linux native CI jobs are configured but have not been executed from this workstation. The five upstream RustSec warnings above remain disclosed. No commits or remote CI runs were created.
