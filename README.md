# ticket-tracker

A generic, markdown-based ticket tracking CLI for backlog governance. Enforces ticket-first development by requiring an active session before code commits.

## Installation

```bash
cargo install --git https://github.com/ottogiron/ticket-tracker
```

This installs the `ticket` binary to `~/.cargo/bin/`.

## Quick Start

```bash
# Create a backlog file (see Backlog Format below)
mkdir -p docs/project/backlog
cat > docs/project/backlog/my-batch.md << 'EOF'
# My Batch

Status: Active
Owner: me
Created: 2026-01-01

## Scope Summary

- Implement feature X

## Ticket FEAT-1 — Add feature X

- Goal: Implement feature X
- In scope:
  - Core implementation
- Out of scope:
  - Performance optimization
- Dependencies: None
- Acceptance criteria:
  - Tests pass
- Verification:
  - cargo test
- Status: Todo

## Execution Metrics

- Ticket: FEAT-1
- Owner: me
- Complexity: M
- Risk: Low
- Start:
- End:
- Duration:
- Notes:
EOF

# Start a ticket session
ticket start FEAT-1

# Check session status
ticket status

# Do your work, commit code...
# The pre-commit hook (if installed) enforces an active session.

# Mark ticket as done
ticket done FEAT-1
```

## Commands

### `ticket start <id>`

Starts a ticket session. Validates the ticket exists in a backlog file, checks required schema fields, and sets status to "In Progress".

```bash
ticket start FEAT-1            # single ticket
ticket start MY-BATCH --batch  # batch session (covers all tickets in the backlog)
```

### `ticket done <id>`

Closes a session. Sets status to "Done", records end time and duration in the execution metrics section.

```bash
ticket done FEAT-1
ticket done MY-BATCH --batch
```

### `ticket status`

Shows all active sessions with elapsed time and derived backlog-aware status labels.

```bash
$ ticket status
Batch: MY-BATCH
Mode: batch
Status: Batch Active
Started: 2026-03-14 14:23 UTC
Elapsed: 01:30:00
File: docs/project/backlog/my-batch.md
```

### `ticket reconcile`

Checks active sessions against backlog truth and reports stale or inconsistent sessions.

```bash
ticket reconcile
ticket reconcile --json
```

Human-readable mode prints a summary plus any problematic sessions. `--json` emits structured diagnostics suitable for repo hooks or automation, and exits non-zero when stale or invalid sessions are found.

### `ticket blocked <id> "<reason>"`

Marks a ticket as blocked and records the reason in the backlog's Tracking Notes section.

```bash
ticket blocked FEAT-1 "Waiting on API design review"
```

### `ticket note <id> "<note>"`

Adds a timestamped note to the backlog's Tracking Notes section.

```bash
ticket note FEAT-1 "Discovered edge case in parser"
```

### `--repo-root`

All commands accept `--repo-root <path>` to override the repository root. When
omitted, the tool auto-detects the main repository root via `git` — this works
correctly from any subdirectory and from linked git worktrees. Falls back to `.`
if `git` is unavailable or the current directory is not inside a repository.

```bash
ticket status --repo-root /path/to/project
```

## Backlog Format

Backlog files are markdown files in `docs/project/backlog/`. Each file represents a batch of related tickets.

### Required Structure

```markdown
# Batch Name

Status: Active
Owner: <owner>
Created: YYYY-MM-DD

## Scope Summary

- Summary of what this batch covers

## Ticket <ID> — <Title>

- Goal: <what this ticket achieves>
- In scope:
  - <item>
- Out of scope:
  - <item>
- Dependencies: <none or ticket IDs>
- Acceptance criteria:
  - <criterion>
- Verification:
  - <command or check>
- Status: Todo

## Execution Metrics

- Ticket: <ID>
- Owner: <name>
- Complexity: <S | M | L | XL>
- Risk: <Low | Medium | High>
- Start:
- End:
- Duration:
- Notes:
```

### Required Ticket Fields

Every ticket section must include these 7 fields (validated by `ticket start`):

| Field | Description |
|-------|-------------|
| `Goal` | What the ticket achieves |
| `In scope` | What's included |
| `Out of scope` | What's excluded |
| `Dependencies` | Prerequisite ticket IDs or "None" |
| `Acceptance criteria` | How to verify success |
| `Verification` | Specific commands or checks |
| `Status` | `Todo`, `In Progress`, `Blocked`, or `Done` |

### Heading Levels

Ticket headings can use any level from H2 to H6 (`##` through `######`). The parser matches flexibly.

## Session Management

Sessions are stored as YAML files in `.sessions/` at the repository root. Each session is keyed by its ticket or batch ID.

```text
.sessions/
  FEAT-1.yaml
  MY-BATCH.yaml
```

### Concurrent Sessions

Multiple sessions can be active simultaneously. This allows working on independent batches in parallel:

```bash
ticket start COMPILER-BATCH --batch
ticket start ORCH-BATCH --batch
ticket status
# Shows both sessions
```

### Session Reconciliation

Session files are operator telemetry, not the source of truth for ticket progress. Backlog status remains authoritative.

Use `ticket reconcile` to detect:

- sessions left open after a ticket is marked `Done`
- sessions pointing at missing backlog files
- sessions whose ticket heading no longer exists
- malformed session YAML

### Legacy Migration

If a legacy `.session` file exists (from the single-session era), it is automatically migrated to `.sessions/<ID>.yaml` on first access.

## Pre-Commit Hook Integration

The ticket system is designed to work with a git pre-commit hook that enforces active sessions for code changes.

### Sample Hook

Create `scripts/hooks/pre-commit`:

```bash
#!/bin/bash
set -e

# Resolve main repo root — works in both main checkout and worktrees.
GIT_COMMON="$(git rev-parse --git-common-dir)"
if [ "$GIT_COMMON" = ".git" ]; then
    REPO_ROOT="$(git rev-parse --show-toplevel)"
else
    REPO_ROOT="$(cd "$GIT_COMMON/.." && pwd)"
fi
SESSION_DIR="$REPO_ROOT/.sessions"

# Check if any valid session exists
has_session=false
if [ -d "$SESSION_DIR" ]; then
    for f in "$SESSION_DIR"/*.yaml; do
        [ -f "$f" ] && has_session=true && break
    done
fi

if [ "$has_session" = true ]; then
    exit 0
fi

# No session — check if code files are staged
for file in $(git diff --cached --name-only --diff-filter=ACMR); do
    case "$file" in
        src/*|tests/*|*.rs)
            echo "ERROR: No active ticket session."
            echo "Run: ticket start <ticket-id>"
            exit 1
            ;;
    esac
done
```

Install with:

```bash
ln -sf ../../scripts/hooks/pre-commit .git/hooks/pre-commit
```

Or add a Makefile target:

```makefile
setup-hooks:
    mkdir -p .git/hooks
    ln -sf ../../scripts/hooks/pre-commit .git/hooks/pre-commit
```

## .gitignore

Add to your `.gitignore`:

```text
.sessions/
.session
```

## How It Works

1. **Backlogs** are markdown files in `docs/project/backlog/` with a structured ticket schema.
2. **`ticket start`** finds the ticket in a backlog file, validates the schema, sets status to "In Progress", records start time, and creates a session file.
3. **The pre-commit hook** checks for an active session before allowing code commits.
4. **`ticket done`** sets status to "Done", calculates duration, records end time, and removes the session.
5. **Batch sessions** cover all tickets in a backlog file — useful for multi-ticket work where you don't want to start/stop individual ticket sessions.

## License

MIT
