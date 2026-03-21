# Contributing to ticket-tracker

Thank you for your interest in contributing! This document covers how to get started.

## Getting Started

1. Fork and clone the repository
2. Install the Rust toolchain (`rustup`)
3. Build: `make build`
4. Run tests: `make test`

## Before Submitting a PR

Run the full verification gate — this matches CI:

```bash
make fmt       # Apply formatting
make verify    # fmt-check + clippy + test + lint-md
```

All four checks must pass.

## Code Style

- Follow `rustfmt` defaults
- Use `Result<T, String>` for recoverable errors
- Use `unwrap()` only in tests
- All clippy warnings are errors (`-D warnings`)
- Test naming: `test_<function>_<scenario>`

## Project Structure

```text
src/
  main.rs      — CLI entry point
  lib.rs       — CLI struct, repo root resolution, command router
  commands.rs  — Command implementations (start, done, status, blocked, note)
  session.rs   — Session lifecycle and I/O (.sessions/ YAML files)
  backlog.rs   — Backlog file parsing and manipulation
```

## Adding a New Command

1. Add the variant to `Commands` enum in `src/lib.rs`
2. Add the handler function in `src/commands.rs`
3. Wire it in the `run()` match in `src/lib.rs`
4. Add tests
5. Update `README.md` with usage examples
6. Add a `CHANGELOG.md` entry under `[Unreleased]`
