# Contributing to Faro ⚓

Thank you for your interest in contributing to **Faro**! We welcome contributions, bug reports, feature requests, and security improvements from the community.

---

## How Can I Contribute?

### Reporting Bugs

Before creating a bug report, please check existing issues to ensure the bug hasn't already been reported. When creating a bug report, please include:

- **A clear and descriptive title**.
- **Steps to reproduce the issue** step-by-step.
- **Expected vs. actual behavior**.
- **Environment details**: Operating system, database engine & version, Faro app version.
- **Relevant error logs** or screenshots (without sensitive database credentials).

### Suggesting Features

Feature requests are always welcome! When proposing a new feature:

- Explain **why** the feature is useful and what problem it solves.
- Describe how you envision the feature working in the UI or backend.
- Consider compatibility across all supported database engines (PostgreSQL, MySQL, MariaDB, SQLite, DuckDB, MongoDB, ClickHouse, MSSQL).

### Submitting Pull Requests

If you'd like to implement a feature or fix a bug:

1. Fork the repository and create a new feature branch off `main`.
2. Ensure your code follows our [coding standards](#development-workflow) and passes all [tests](#testing).
3. Update relevant documentation and `CHANGELOG.md`.
4. Open a Pull Request targeting the `main` branch.

---

## Development Environment Setup

### Prerequisites

Make sure you have installed:

- **Node.js**: `v20.0.0` or higher (v22 recommended)
- **pnpm**: `v11.17.0` or higher (`corepack enable` or `npm i -g pnpm`)
- **Rust**: `1.85` or higher (`rustup update stable`)
- **Docker & Docker Compose**: Required for running integration tests against real database engines.
- **Tauri Prerequisites**: Refer to [Tauri v2 Documentation](https://v2.tauri.app/start/prerequisites/) for OS-specific native build tools (e.g. `libwebkit2gtk-4.1-dev` on Linux, Xcode command line tools on macOS).

### Local Setup

1. **Clone your fork**:
   ```bash
   git clone https://github.com/YOUR-USERNAME/faro.git
   cd faro
   ```

2. **Install Node.js dependencies**:
   ```bash
   pnpm install
   ```

3. **Start the application in development mode**:
   ```bash
   pnpm tauri dev
   ```

---

## Development Workflow

### Frontend Guidelines

- **Stack**: React 19, TypeScript 5.9, Vite 8, Tailwind CSS v4, Zustand 5, TanStack Virtual v3, CodeMirror 6.
- **TypeScript**: Strict type-checking is enforced (`pnpm typecheck`). Avoid using `any` types; define explicit interfaces or types.
- **State Management**: Use Zustand stores in `src/state/` for global app state, keeping transient component state local. Write unit tests for custom store actions (`tests/state/*.test.ts`).
- **UI Components**: Keep components functional, accessible, and responsive. Modularize complex features under `src/features/`. Include component unit tests with `@testing-library/react` under `tests/` (`*.test.tsx`).
- **Command Palette**: When adding new top-level features or global actions, register corresponding commands in `src/features/palette/CommandPalette.tsx`.

### Backend Guidelines (Rust)

- **Stack**: Rust 2021 edition (MSRV 1.85), Tauri v2.11, Tokio async runtime, `sqlx` 0.9, `rusqlite` 0.39, `duckdb` 1, `mongodb` 3, `tiberius` 0.12, `tauri-plugin-updater`.
- **Security & Validation**:
  - **Read-Only Mode**: Any new SQL execution paths must respect the strict read-only validator (`src-tauri/src/sql.rs`) when operating under read-only connections.
  - **Export Sanitization**: Any new export routines (CSV/TSV/delimited) must sanitize potential spreadsheet formula injection characters (`=`, `+`, `-`, `@`, `\t`, `\r`) by escaping them.
  - **SSL/TLS Drivers**: When adding or updating database drivers, support SSL connection modes and custom CA/client certificate configurations where applicable.
- **Error Handling**: Use the custom error types in `src-tauri/src/error.rs` powered by `thiserror`. Do not panic (`unwrap()`) in production IPC paths; return structured `Result<T, FaroError>`.
- **Async & Concurrency**: Avoid blocking execution on the main UI loop or Tokio worker threads. Use `tokio::task::spawn_blocking` for CPU-heavy disk or parsing work if necessary.

### Linting and Formatting

Faro uses **Biome** for JavaScript/TypeScript formatting and linting, and standard **rustfmt** / **clippy** for Rust.

```bash
# Check TypeScript types
pnpm typecheck

# Lint frontend files with Biome
pnpm lint

# Automatically fix linting and formatting issues
pnpm lint:fix
pnpm format

# Check Rust formatting and clippy lints
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --no-default-features --no-deps -- -D warnings
```

---

## Testing

### Frontend Unit Tests

Frontend tests are written with **Vitest** and located in the dedicated `tests/` directory (e.g., `tests/lib/*.test.ts`, `tests/features/*.test.tsx`).

```bash
# Run tests once
pnpm test

# Run tests in watch mode
pnpm test:watch
```

### Backend Integration Tests

Backend integration tests live in `src-tauri/tests/`. To run tests against real database engines:

1. **Start test containers**:
   ```bash
   docker compose -f docker-compose.test.yml up -d
   ```

2. **Seed database fixtures**:
   ```bash
   ./scripts/seed.sh
   ```

3. **Execute Rust test suite**:
   ```bash
   # Full test suite (includes bundled DuckDB compilation)
   cargo test --manifest-path src-tauri/Cargo.toml

   # Fast iteration mode (skips compiling bundled DuckDB C++ amalgamation)
   cargo test --manifest-path src-tauri/Cargo.toml --no-default-features
   ```

---

## Pull Request Process

1. **Branch Naming**: Use descriptive branch names:
   - `feature/short-description`
   - `fix/short-description`
   - `docs/short-description`
2. **Commit Messages**: Follow [Conventional Commits](https://www.conventionalcommits.org/):
   - `feat: add support for connection read-only enforcement`
   - `fix: resolve CSV export encoding issue`
   - `docs: update setup instructions`
   - `refactor: clean up schema cache store`
   - `test: add integration test for MSSQL decimal types`
3. **Verification Checklist**: Before submitting your PR, verify that:
   - [ ] `pnpm typecheck` succeeds without errors.
   - [ ] `pnpm lint` reports no warnings or errors.
   - [ ] `cargo fmt --check --manifest-path src-tauri/Cargo.toml` passes.
   - [ ] `cargo clippy --manifest-path src-tauri/Cargo.toml --no-default-features --no-deps -- -D warnings` reports no warnings.
   - [ ] `pnpm test` passes cleanly.
   - [ ] `cargo test --manifest-path src-tauri/Cargo.toml --no-default-features` passes cleanly.
   - [ ] `CHANGELOG.md` has been updated under `[Unreleased]`.

---

Happy coding! ⚓