# AGENTS.md — ticket-tracker

Operational guide for agents working in this repository.

## Project Overview

ticket-tracker is a generic, markdown-based ticket tracking CLI for backlog governance. It enforces ticket-first development by requiring an active session before code commits.

This is a standalone repository (`ottogiron/ticket-tracker`).

## Project Principles

- Keep it simple — this is a small, focused CLI tool.
- Correctness over features. The tool must never corrupt backlog files or lose session state.
- Active development, no backward-compatibility burden. Pre-v1, rapid iteration.

## Module Overview

- `src/main.rs` — CLI entry point
- `src/lib.rs` — CLI struct, repo root resolution (`resolve_repo_root_from`), command router
- `src/commands.rs` — command implementations (start, done, status, blocked, note)
- `src/session.rs` — session lifecycle and I/O (`.sessions/` YAML files)
- `src/backlog.rs` — backlog file parsing, schema validation, and field manipulation

## Build Commands

```bash
make build       # cargo build
make test        # cargo test
make fmt         # cargo fmt --all
make verify      # fmt-check + clippy + test + lint-md (matches CI)
```

## Code Style

- Follow `rustfmt` defaults.
- Use `Result<T, String>` for recoverable errors.
- Use `unwrap()` only in tests.
- All clippy warnings are errors (`-D warnings`).
- Test naming: `test_<function>_<scenario>`.

## Quality Gates

```bash
make verify    # always run before pushing
```

This runs `fmt-check` + `clippy` + `test` + `lint-md`. All four checks must pass.

## Key Design Details

### Repo Root Resolution

`resolve_repo_root_from(working_dir)` uses `git rev-parse --git-common-dir` to find the main repository root. This works from subdirectories and linked git worktrees. Falls back to `working_dir` if not inside a git repo.

### Session Storage

Sessions are YAML files in `{repo_root}/.sessions/{TICKET_ID}.yaml`. Multiple sessions can be active concurrently. The pre-commit hook (in consumer repos) validates that at least one active session exists before allowing code commits.

### Backlog Files

Markdown files in `docs/project/backlog/` with a structured ticket schema. The tool searches all `.md` files under this directory for ticket headings matching `## Ticket <ID>` (H2–H6 flexible).

### Testing

- Unit tests use `tempfile::TempDir` for isolation.
- Do NOT use `std::env::set_current_dir` in tests — it mutates process-global state and causes races. Pass paths as arguments instead.
- The worktree integration test creates a real git repo + worktree in a tempdir.
