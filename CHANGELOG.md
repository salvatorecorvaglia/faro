# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **SSL/TLS Connection Configurations**: Added support for custom CA certificates, client certificates, client keys, and SSL connection modes (disable, require, verify-ca, verify-full) across PostgreSQL, MySQL, MariaDB, and MSSQL.
- **Command Palette**: Added an interactive keyboard-first command palette (`Cmd+K` / `Ctrl+K`) for quick actions, view navigation, and tab switching.
- **Frontend Component & Store Unit Tests**: Added Vitest unit test suites for UI components, Zustand state stores, and feature dialogs.

### Changed
- **App Icon Relocation**: Migrated application icon to `resources/faro.png` for better project structure.
- **SQLite Foreign Key Introspection**: Refactored schema reflection to correctly group multi-column compound foreign keys.
- **Test Suite Restructuring**: Relocated frontend unit tests (`*.test.ts`, `*.test.tsx`) into the root `tests/` directory and updated Vitest and TypeScript compilation configurations.

### Fixed
- **Decimal Precision & Normalization on PostgreSQL and MySQL**: `numeric` and `DECIMAL` values are now decoded through `BigDecimal` instead of `rust_decimal` (resolving its ~28-significant-digit limitation) and normalized to strip trailing fractional zeros without losing numeric precision.
- **Text Filters on ClickHouse**: Browse filters using "contains" and "starts with" no longer emit a `LIKE ... ESCAPE` clause on ClickHouse, which rejects it as a syntax error. Wildcards are still escaped — ClickHouse treats backslash as the escape character by default.

### Security
- **CSV Formula Injection Sanitization**: Implemented automatic formula escaping (`'`, `=`, `+`, `-`, `@`, `\t`, `\r`) during CSV exports to prevent spreadsheet macro execution vulnerabilities.
- **Enhanced Read-Only Validation**: Expanded the SQL parser and query validator to strictly detect and block data-modifying statements and DDL operations in read-only connection mode.

## [1.0.0] - 2026-08-09

### Added

- First implementation of Faro.