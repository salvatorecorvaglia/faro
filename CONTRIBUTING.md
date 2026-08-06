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

- **Node.js**: `v18.0.0` or higher
- **pnpm**: `v11.17.0` or higher (`corepack enable` or `npm i -g pnpm`)
- **Rust**: `1.85` or higher (`rustup update stable`)
- **Docker & Docker Compose**: Required for running integration tests against real database engines.
- **Tauri Prerequisites**: Refer to [Tauri v2 Documentation](https://v2.tauri.app/start/prerequisites/) for OS-specific native build tools.

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

- **Stack**: React 19, TypeScript, Vite, Tailwind CSS v4, Zustand.
- **TypeScript**: Strict type-checking is enforced. Avoid using `any` types; define explicit interfaces or types.
- **State Management**: Use Zustand stores in `src/state/` for global app state, keeping transient component state local.
- **UI Components**: Keep components functional, accessible, and responsive. Modularize complex features under `src/features/`.

### Backend Guidelines (Rust)

- **Stack**: Rust 2021 edition (MSRV 1.85), Tauri v2, Tokio async runtime, `sqlx`, `rusqlite`, `duckdb`, `mongodb`, `tiberius`.
- **Error Handling**: Use the custom error types in `src-tauri/src/error.rs` powered by `thiserror`. Do not panic (`unwrap()`) in production IPC paths; return structured `Result<T, FaroError>`.
- **Async & Concurrency**: Avoid blocking execution on the main UI loop or blocking Tokio worker threads. Use `tokio::task::spawn_blocking` for CPU-heavy disk or parsing work if necessary.

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

# Format and lint Rust backend
cd src-tauri
cargo fmt --check
cargo clippy
```

---

## Testing

### Frontend Unit Tests

Frontend tests are written with **Vitest** and placed alongside source files (e.g., `*.test.ts`).

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
   # Full test suite
   cargo test --manifest-path src-tauri/Cargo.toml

   # Fast iteration mode (excludes DuckDB C++ compilation)
   cargo test --manifest-path src-tauri/Cargo.toml --no-default-features
   ```

---

## Pull Request Process

1. **Branch Naming**: Use descriptive branch names:
   - `feature/short-description`
   - `fix/short-description`
   - `docs/short-description`
2. **Commit Messages**: Follow [Conventional Commits](https://www.conventionalcommits.org/):
   - `feat: add support for custom connection timeouts`
   - `fix: resolve CSV export encoding issue`
   - `docs: update setup instructions`
   - `refactor: clean up schema cache store`
   - `test: add integration test for MSSQL decimal types`
3. **Verification Checklist**: Before submitting your PR, verify that:
   - [ ] `pnpm typecheck` succeeds without errors.
   - [ ] `pnpm lint` and `cargo clippy` report no warnings/errors.
   - [ ] `pnpm test` and `cargo test --no-default-features` pass cleanly.
   - [ ] `CHANGELOG.md` has been updated under `[Unreleased]`.

---

Happy coding! ⚓