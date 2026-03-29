# Changelog

All notable changes to ticket-tracker are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `ticket import` command — scans `docs/project/backlog/*.md` and populates SQLite with tickets, statuses, execution metrics, tracking notes, and closure evidence; idempotent and safe to re-run
- Markdown lint (`markdownlint-cli2`) to `make verify` quality gate
- `.markdownlint-cli2.jsonc` and `.markdownlintignore` config files

### Changed

- `--repo-root` auto-detects the main git repository root via `git rev-parse --git-common-dir`. Works correctly from subdirectories and linked git worktrees. Falls back to `.` if not inside a git repo.

### Fixed

- Test race condition: `test_resolve_repo_root_from_returns_existing_path` no longer mutates process-global CWD — uses `CARGO_MANIFEST_DIR` instead

## [0.1.0] — 2026-03-21

### Added

- `ticket start <id>` — start a ticket session with schema validation
- `ticket start <id> --batch` — start a batch session covering all tickets in a backlog file
- `ticket done <id>` — close a ticket session, record duration and end time
- `ticket done <id> --batch` — close a batch session
- `ticket status` — list all active sessions with elapsed time
- `ticket blocked <id> "<reason>"` — mark a ticket as blocked
- `ticket note <id> "<note>"` — add a timestamped tracking note
- `--repo-root <path>` global flag to override repository root
- Backlog file discovery via glob in `docs/project/backlog/`
- Required ticket schema validation (7 fields: Goal, In scope, Out of scope, Dependencies, Acceptance criteria, Verification, Status)
- Flexible heading levels (H2–H6) for ticket sections
- Concurrent session support via `.sessions/` directory
- Legacy `.session` file auto-migration
- Execution metrics auto-population (start time, end time, duration)
- Pre-commit hook integration for backlog governance
