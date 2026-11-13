# Safety regression baseline

Run the passing checks before and after each remediation phase:

```sh
pnpm lint
pnpm typecheck
pnpm test:coverage
pnpm build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --no-default-features --manifest-path src-tauri/Cargo.toml --no-deps -- -D warnings
cargo test --locked --no-default-features --manifest-path src-tauri/Cargo.toml --lib
```

## Executable regressions

- `BackupDialog.test.tsx`: same-named tables in different schemas select independently.
- `TableTab.test.tsx`: an old browse response cannot replace a newer response after remount.
- Registry unit tests: submissions on one connection wait; another connection proceeds; queued cancellation releases its waiter.
- Transfer unit tests: two outputs targeting one destination have separate temporary files, and cleanup cannot delete another writer's file.

The quoted-function, transaction-ownership, and stale-update regressions now run in the normal suite. None of these cases is ignored:

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --test safety_regressions --test cancellation
```

The integration cases create their own temporary SQLite/DuckDB databases. Stale-update and delete tests change rows through an independent connection. Updates carry original values for edited columns; deletes carry the full row. Snapshot checks reject missing or unrepresentable originals and use exact text comparison independently of case-insensitive collations. JSON/arrays/unreadable values and omitted large binary values are refused where they cannot be compared safely. Finite floating-point originals are compared using the engine's numeric representation.

Cancellation tests cover a pre-cancelled import with `Done` already queued, cancellation while waiting for producer input, interruption of native DuckDB work, rollback and session reuse, and cancellation after a completed commit. SQL Server drains an executing statement before checking cancellation and rolling back; cancellation does not claim to kill server work immediately. Once commit has started, its result is awaited and reported.

Transfer tests cover independent temporary files, failed replacement preserving an existing destination, successful overwrite, cleanup, and refusal of injected symlinks. Outputs are exclusively created inside a private temporary directory beside the destination; each writer owns its open handle.

Credential tests inject failure/success callbacks without touching the developer's real keychain. They verify deletion errors, absent entries, failed replacement retaining fallback, and successful persistent replacement clearing fallback.

Quit-hook tests dispatch the native event bridge and verify listener registration, clean exit, declining with dirty tabs, repeated requests, running-operation warnings, and cleanup. Updater tests verify declining a restart preserves edits. Registry tests keep cancelled operations visible until rollback completes and prevent an older guard from removing a replacement operation.

## Phase 4 resource and transfer checks

- Result collectors account for text, binary values, nested JSON/arrays, and column metadata. A single oversized SQLite cell is rejected with a structured `resourceLimit` error and the connection remains usable.
- Cache and tab-store tests cover byte-based eviction and explicit release. Editor SQL survives eviction. Cache limits do not include separate in-flight exports that still own a shared result handle.
- ClickHouse decoder tests split UTF-8 and JSON metadata across chunks, reject incomplete metadata, and stop before buffering surplus rows. Requests enable `cancel_http_readonly_queries_on_client_close`; server/version/network behavior still needs the live gate below. This does not promise cancellation of writes.
- JSON importer tests retain the union of columns, stop preview batches, preserve producer errors, and compare streamed output with the existing reader. CSV/JSON batch sizes are bounded by bytes as well as rows. XLSX preflight rejects a tiny archive describing an enormous sparse dense range before allocation.
- The disk actor accepts only two 64 KiB chunks at a time, runs blocking file I/O on the blocking pool, and publishes only after an explicit commit. Existing CSV/TSV/JSON/SQL/XLSX round trips and cancellation tests exercise output integrity.
- A SQLite WAL backup test changes a later page through a second connection after the first page is emitted; the dump retains the earlier snapshot. A DuckDB test changes data through the editor session and verifies snapshot reads remain unchanged.
- Progress tests reject events from another operation and queued events from a disposed subscription. Switching operation IDs clears old progress.

Defaults: estimated result 32 MiB; run 64 MiB; cache 128 MiB; tabs 128 MiB; materialized workbook/restore 64 MiB; editor submission 8 MiB/10,000 statements. Generated table reads reduce their page size on resource-limit errors. These are retained-data policies, not hard RSS limits; one native row/frame or parser value can be allocated before it is checked.

## Native and browser validation

`pnpm test:browser` exercises layout, focus restoration, keyboard input, worker sorting and two-dimensional virtualization in Chromium and WebKit. `pnpm test:browser:startup` serves production assets and verifies that the editor and formatter are deferred.

`pnpm test:desktop` builds a separate debug smoke app with bundled frontend assets and Tauri's production custom protocol. Its private settings/database files and incognito webviews do not open saved user connections. It exercises native close/quit decline and acceptance, staged edits, shared restart confirmation, snapshot backup, and active import/restore warnings and cancellation. macOS additionally checks a disposable locked keychain. See `PHASE5_VALIDATION.md` for recorded results and scope. CI includes native smoke and file-publication checks on macOS, Windows and Linux; configuring those jobs is not evidence they have already run remotely.

## Live database gate

Start the disposable services in `docker-compose.test.yml`, seed them with `scripts/seed.sh`, then run:

```sh
FARO_REQUIRE_LIVE=1 cargo test --locked --manifest-path src-tauri/Cargo.toml --test live_engines
```

The mandatory flag prevents missing servers from producing a misleading passing run. A no-default-features build excludes DuckDB and does not validate the shipped default-feature configuration. The separate desktop harness covers native routing and macOS keychain behavior; Windows/Linux runtime results require those CI jobs or equivalent local machines.
