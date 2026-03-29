# Tooling Improvements

Status: Active
Owner: otto
Created: 2026-03-29

## Scope Summary

- Improve session visibility and reconciliation in the `ticket` CLI

## Ticket RECON-1 — Session reconciliation and status diagnostics

- Goal: Surface stale or inconsistent sessions directly in `ticket` so operators and consumer repos do not need to rediscover drift manually.
- In scope:
  - Add a reconciliation pass for active sessions against backlog truth
  - Add a `ticket reconcile` command with human-readable and JSON output
  - Update `ticket status` to show derived session state instead of always reporting `In Progress`
  - Preserve and ship the existing execution-metrics formatting fix in `src/backlog.rs`
- Out of scope:
  - Automatically deleting or closing stale sessions
  - Inferring exact staged-file-to-ticket ownership
  - Changing consumer-repo hooks in this ticket
- Dependencies: None
- Acceptance criteria:
  - `ticket reconcile` reports stale, missing, and invalid sessions
  - `ticket reconcile --json` emits structured diagnostics suitable for automation
  - `ticket status` shows derived session status labels
  - Existing valid active ticket and batch sessions continue to work
  - `make verify` passes
- Verification:
  - `make verify`
  - Manual: simulate a session with backlog status `Done` and confirm `ticket reconcile` flags it
- Status: In Progress

## Tracking Notes

## Execution Metrics

- Ticket: RECON-1
- Owner: otto
- Complexity: S
- Risk: Low
- Start:
- End:
- Duration:
- Notes:
- Start: 2026-03-29 03:29 UTC
