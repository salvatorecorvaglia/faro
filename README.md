# Faro ⚓

**Fast, clean, cross-platform database client**

**Faro** is a lightweight, high-performance desktop application that provides a unified, intuitive interface to connect, query, edit, import/export, and manage relational, analytical, and document databases without requiring external command-line utilities.

---

## ✨ Features

- ⚡ **Multi-Engine Support**: Native drivers for **PostgreSQL**, **MySQL**, **MariaDB**, **SQLite**, **DuckDB**, **MongoDB**, **ClickHouse**, and **Microsoft SQL Server (MSSQL)**.
- 🛡️ **Read-Only Mode & Safety**: Connection-level read-only enforcement to prevent accidental data modifications or destructive DDL queries on production environments.
- 🔒 **Secure Credential Storage**: Native OS password manager integration (macOS Keychain, Windows Credential Manager, Linux Secret Service via system `keyring`).
- 📝 **Advanced SQL Editor**: Built with CodeMirror 6 featuring schema-aware autocompletion, query formatting (`sql-formatter`), tabbed workflows, and multi-query execution.
- 📊 **Virtualized Data Grid**: Lightning-fast table rendering for massive datasets using `@tanstack/react-virtual`, complete with inline DML editing, dynamic filtering, and column sorting.
- 📂 **Flexible Import, Export & Transfer**: Import and export data seamlessly across CSV, Excel (`.xlsx`), JSON, and raw SQL dumps, or stream data directly between databases.
- ⚙️ **Embedded Databases**: Full zero-config support for embedded SQLite (`rusqlite`) and analytical DuckDB (`duckdb-rs`) workloads directly inside the client process.
- 🔄 **Automatic Application Updates**: Built-in update notifications and one-click upgrades powered by `tauri-plugin-updater`.
- 🎨 **Modern Interface**: Designed with Tailwind CSS v4 and dynamic resizable panels (`react-resizable-panels`) for an uncluttered user experience.

---

## 🗄️ Supported Database Engines

| Database Engine | Type | Connection Protocol / Driver | Test Suite Port |
| :--- | :--- | :--- | :--- |
| **PostgreSQL** | Relational | Native (`sqlx-postgres`) | `55432` |
| **MySQL** | Relational | Native (`sqlx-mysql`) | `53306` |
| **MariaDB** | Relational | Native (`sqlx-mysql`) | `53307` |
| **SQLite** | Embedded Relational | Bundled (`rusqlite` / `sqlx-sqlite`) | Local file |
| **DuckDB** | Embedded Analytical | Bundled (`duckdb-rs`) | Local file |
| **MongoDB** | Document / NoSQL | Native (`mongodb` Rust driver + BSON parser) | `57017` |
| **ClickHouse** | Analytical Columnar | HTTP Interface (`reqwest`) | `58123` |
| **SQL Server (MSSQL)** | Relational | Native TDS (`tiberius` + `bb8`) | `51433` |

---

## 🚀 Quick Start

### Prerequisites

Ensure you have the following installed on your machine:

- **Node.js**: `v20.0.0` or higher (v22 recommended)
- **pnpm**: `v11.17.0`+ (`corepack enable` or `npm i -g pnpm`)
- **Rust Toolchain**: `1.85`+ (`rustup update stable`)
- **System Dependencies for Tauri v2**: Refer to the official [Tauri Prerequisites Guide](https://v2.tauri.app/start/prerequisites/) for your operating system (macOS Xcode tools, Linux `libwebkit2gtk-4.1-dev`, or Windows C++ Build Tools).

### Installation & Local Development

1. **Clone the repository**:
   ```bash
   git clone https://github.com/salvatorecorvaglia/faro.git
   cd faro
   ```

2. **Install frontend dependencies**:
   ```bash
   pnpm install
   ```

3. **Launch Faro in development mode**:
   ```bash
   pnpm tauri dev
   ```
   *This starts the Vite development server on `http://localhost:1420` and launches the native desktop client window with Hot Module Replacement (HMR).*

---

## 🧪 Testing & Verification

Faro includes a Docker Compose environment containing pre-configured instances of all supported database engines for end-to-end integration testing.

### Running Test Databases

1. **Spin up the test containers**:
   ```bash
   docker compose -f docker-compose.test.yml up -d
   ```

2. **Seed the database fixtures**:
   ```bash
   ./scripts/seed.sh
   ```

### Running Test Suites

- **Run Frontend Tests (Vitest)**:
  ```bash
  pnpm test
  ```

- **Run Backend Integration Tests (Rust)**:
  ```bash
  # Run all Rust unit and integration tests (includes DuckDB)
  cargo test --manifest-path src-tauri/Cargo.toml

  # Fast iteration mode (skips compiling bundled DuckDB C++ amalgamation)
  cargo test --manifest-path src-tauri/Cargo.toml --no-default-features
  ```

- **Linting & Code Quality**:
  ```bash
  # Check TypeScript types
  pnpm typecheck

  # Lint frontend & configuration files with Biome
  pnpm lint

  # Format frontend files automatically
  pnpm format

  # Check Rust formatting and clippy lints
  cargo fmt --check --manifest-path src-tauri/Cargo.toml
  cargo clippy --manifest-path src-tauri/Cargo.toml --no-default-features --no-deps -- -D warnings
  ```

---

## 🛠️ Build for Production

To compile a production release bundle (installers, disk images, or executable packages for macOS, Linux, or Windows):

```bash
pnpm tauri build
```

The compiled binaries will be output to `src-tauri/target/release/bundle/`.

---

## 🤝 Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## 📜 Changelog

Detailed release history and version changes can be found in [CHANGELOG.md](CHANGELOG.md).

## 🔐 Security

If you discover a security vulnerability, please see our [Security Policy](SECURITY.md).

## 📝 License

Distributed under the MIT License. See [LICENSE](LICENSE) for more information.

---

**Author**: [Salvatore Corvaglia](https://github.com/salvatorecorvaglia)