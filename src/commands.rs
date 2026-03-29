use crate::store::{self, Store, Ticket, TicketStatus};
use crate::{
    Backlog, DerivedStatus, ReconcileReport, ReconcileSummary, Session, SessionDiagnostic,
    SESSION_DIR,
};
use chrono::{NaiveDateTime, Utc};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

fn format_duration(duration: chrono::Duration) -> String {
    let total_secs = duration.num_seconds();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

fn status_label(status: &DerivedStatus) -> &'static str {
    match status {
        DerivedStatus::Active => "In Progress",
        DerivedStatus::BatchActive => "Batch Active",
        DerivedStatus::StaleDone => "Stale (Done in backlog)",
        DerivedStatus::StaleBlocked => "Stale (Blocked in backlog)",
        DerivedStatus::StatusMismatch => "Mismatch (Backlog status differs)",
        DerivedStatus::MissingBacklog => "Invalid (Missing backlog file)",
        DerivedStatus::MissingTicket => "Invalid (Missing ticket heading)",
        DerivedStatus::InvalidSession => "Invalid session",
    }
}

fn render_status(report: &ReconcileReport) -> String {
    if report.sessions.is_empty() {
        return "No active sessions\nRun 'ticket start <ticket-id>' to begin work.\n".to_string();
    }

    let mut output = String::new();
    for (i, session) in report.sessions.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }

        if session.mode == "batch" {
            output.push_str(&format!("Batch: {}\n", session.ticket_id));
            output.push_str("Mode: batch\n");
        } else {
            output.push_str(&format!("Ticket: {}\n", session.ticket_id));
            output.push_str(&format!("Mode: {}\n", session.mode));
        }

        output.push_str(&format!(
            "Status: {}\n",
            status_label(&session.derived_status)
        ));
        if let Some(start) = session.start.as_ref() {
            let elapsed = Utc::now() - *start;
            output.push_str(&format!(
                "Started: {}\n",
                start.format("%Y-%m-%d %H:%M UTC")
            ));
            output.push_str(&format!("Elapsed: {}\n", format_duration(elapsed)));
        }
        output.push_str(&format!("File: {}\n", session.file.display()));
        if let Some(backlog_status) = &session.backlog_status {
            output.push_str(&format!("Backlog Status: {}\n", backlog_status));
        }
        if let Some(message) = &session.message {
            output.push_str(&format!("Message: {}\n", message));
        }
    }

    if report.sessions.len() > 1 {
        output.push('\n');
        output.push('\n');
        output.push_str(&format!("{} active sessions\n", report.sessions.len()));
    }

    output
}

fn render_reconcile(report: &ReconcileReport) -> String {
    if report.sessions.is_empty() {
        return "No active sessions to reconcile.\n".to_string();
    }

    let mut output = String::new();
    output.push_str("Session Reconciliation Summary\n");
    output.push_str(&format!("  Total: {}\n", report.summary.total_sessions));
    output.push_str(&format!("  Active: {}\n", report.summary.active));
    output.push_str(&format!(
        "  Batch Active: {}\n",
        report.summary.batch_active
    ));
    output.push_str(&format!("  Stale Done: {}\n", report.summary.stale_done));
    output.push_str(&format!(
        "  Stale Blocked: {}\n",
        report.summary.stale_blocked
    ));
    output.push_str(&format!(
        "  Status Mismatch: {}\n",
        report.summary.status_mismatch
    ));
    output.push_str(&format!(
        "  Missing Backlog: {}\n",
        report.summary.missing_backlog
    ));
    output.push_str(&format!(
        "  Missing Ticket: {}\n",
        report.summary.missing_ticket
    ));
    output.push_str(&format!(
        "  Invalid Session: {}\n",
        report.summary.invalid_session
    ));

    let problems: Vec<_> = report
        .sessions
        .iter()
        .filter(|session| {
            !matches!(
                session.derived_status,
                DerivedStatus::Active | DerivedStatus::BatchActive
            )
        })
        .collect();

    if problems.is_empty() {
        output.push_str("\nAll sessions are consistent.\n");
        return output;
    }

    output.push_str("\nProblems\n");
    for session in problems {
        output.push_str(&format!(
            "- {} ({})\n",
            session.ticket_id,
            status_label(&session.derived_status)
        ));
        output.push_str(&format!("  File: {}\n", session.file.display()));
        if let Some(backlog_status) = &session.backlog_status {
            output.push_str(&format!("  Backlog Status: {}\n", backlog_status));
        }
        if let Some(message) = &session.message {
            output.push_str(&format!("  Message: {}\n", message));
        }
    }

    output
}

/// Open the SQLite store, migrating any YAML sessions from `.sessions/` when
/// the database does not yet exist.
fn open_store_with_migration(repo_root: &Path) -> Result<store::Store, String> {
    let db_path = repo_root.join(".ticket").join("state.db");
    let yaml_dir = repo_root.join(SESSION_DIR);

    // Collect YAML sessions before opening the store (which creates the DB file).
    let yaml_sessions = if !db_path.exists() && yaml_dir.exists() {
        Session::list_all(repo_root)?
    } else {
        vec![]
    };

    let store = store::Store::open(repo_root)?;

    if !yaml_sessions.is_empty() {
        eprintln!(
            "Migrating {} YAML session(s) to SQLite...",
            yaml_sessions.len()
        );
        for session in &yaml_sessions {
            let backlog_str = session.file.to_string_lossy().into_owned();
            let ts = session.start.timestamp();
            if store.get_ticket(&session.ticket_id)?.is_none() {
                store.insert_ticket(&store::Ticket {
                    ticket_id: session.ticket_id.clone(),
                    backlog: backlog_str,
                    title: session.ticket_id.clone(),
                    spec_file: None,
                    created_at: ts,
                })?;
            }
            store.insert_session(&session.ticket_id, &session.mode, None, ts)?;
        }
        eprintln!("Migration complete.");
    }

    Ok(store)
}

/// Build a ReconcileReport by validating SQLite active sessions against
/// backlog .md headings.
fn reconcile_from_store(store: &store::Store, repo_root: &Path) -> Result<ReconcileReport, String> {
    let sessions = store.active_sessions()?;
    let mut diagnostics: Vec<SessionDiagnostic> = Vec::new();

    for session in &sessions {
        let backlog_path = store
            .get_ticket(&session.ticket_id)?
            .map(|t| PathBuf::from(&t.backlog))
            .unwrap_or_else(|| PathBuf::from(format!("unknown/{}.md", session.ticket_id)));

        let start = chrono::DateTime::from_timestamp(session.started_at, 0);

        let resolved = if backlog_path.is_absolute() {
            backlog_path
        } else {
            repo_root.join(&backlog_path)
        };

        let diagnostic = if !resolved.exists() {
            SessionDiagnostic {
                ticket_id: session.ticket_id.clone(),
                mode: session.mode.clone(),
                file: resolved,
                start,
                derived_status: DerivedStatus::MissingBacklog,
                backlog_status: None,
                message: Some("Referenced backlog file does not exist".to_string()),
            }
        } else if session.mode == "batch" {
            SessionDiagnostic {
                ticket_id: session.ticket_id.clone(),
                mode: session.mode.clone(),
                file: resolved,
                start,
                derived_status: DerivedStatus::BatchActive,
                backlog_status: None,
                message: None,
            }
        } else {
            // Validate that the ticket heading still exists in the backlog file.
            match Backlog::read(&resolved).and_then(|b| b.get_status(&session.ticket_id)) {
                Ok(_) => SessionDiagnostic {
                    ticket_id: session.ticket_id.clone(),
                    mode: session.mode.clone(),
                    file: resolved,
                    start,
                    derived_status: DerivedStatus::Active,
                    backlog_status: None,
                    message: None,
                },
                Err(e) => SessionDiagnostic {
                    ticket_id: session.ticket_id.clone(),
                    mode: session.mode.clone(),
                    file: resolved,
                    start,
                    derived_status: DerivedStatus::MissingTicket,
                    backlog_status: None,
                    message: Some(e),
                },
            }
        };

        diagnostics.push(diagnostic);
    }

    diagnostics.sort_by(|a, b| a.ticket_id.cmp(&b.ticket_id));

    let mut summary = ReconcileSummary::default();
    for d in &diagnostics {
        summary.total_sessions += 1;
        match d.derived_status {
            DerivedStatus::Active => summary.active += 1,
            DerivedStatus::BatchActive => summary.batch_active += 1,
            DerivedStatus::StaleDone => summary.stale_done += 1,
            DerivedStatus::StaleBlocked => summary.stale_blocked += 1,
            DerivedStatus::StatusMismatch => summary.status_mismatch += 1,
            DerivedStatus::MissingBacklog => summary.missing_backlog += 1,
            DerivedStatus::MissingTicket => summary.missing_ticket += 1,
            DerivedStatus::InvalidSession => summary.invalid_session += 1,
        }
    }

    let ok = diagnostics.iter().all(|d| {
        matches!(
            d.derived_status,
            DerivedStatus::Active | DerivedStatus::BatchActive
        )
    });

    Ok(ReconcileReport {
        ok,
        sessions: diagnostics,
        summary,
    })
}

pub fn start(repo_root: &Path, ticket_id: &str) -> Result<(), String> {
    let ticket_id = ticket_id.to_uppercase();

    let store = open_store_with_migration(repo_root)?;

    let active = store.active_sessions()?;
    if active.iter().any(|s| s.ticket_id == ticket_id) {
        println!("Ticket {} is already in progress", ticket_id);
        return Ok(());
    }

    // Done-guard: SQLite is authoritative for tickets managed by the new system.
    let sqlite_status = store.get_ticket_status(&ticket_id)?;
    if let Some(ref ts) = sqlite_status {
        if ts.status == "done" {
            return Err(format!(
                "Ticket {} is already Done. Create a new ticket or re-open this one.",
                ticket_id
            ));
        }
    }

    let backlog_path = Session::find_backlog_file(repo_root, &ticket_id)?;
    let backlog = Backlog::read(&backlog_path)?;
    backlog.validate_required_ticket_schema(&ticket_id)?;

    // Pre-migration fallback: .md status is only checked when SQLite has no record.
    // New-format tickets omit the Status field; get_status() will return Err,
    // which we treat as "not done in backlog" and allow the start to proceed.
    if sqlite_status.is_none() {
        if let Ok(md_status) = backlog.get_status(&ticket_id) {
            if md_status == "Done" {
                return Err(format!(
                    "Ticket {} is already Done. Create a new ticket or re-open this one.",
                    ticket_id
                ));
            }
        }
    }

    let now = Utc::now();
    let now_ts = now.timestamp();
    let backlog_str = backlog_path.to_string_lossy().into_owned();

    if store.get_ticket(&ticket_id)?.is_none() {
        store.insert_ticket(&store::Ticket {
            ticket_id: ticket_id.clone(),
            backlog: backlog_str,
            title: ticket_id.clone(),
            spec_file: None,
            created_at: now_ts,
        })?;
    }

    store.upsert_ticket_status(&store::TicketStatus {
        ticket_id: ticket_id.clone(),
        status: "in_progress".to_string(),
        blocked_reason: None,
        started_at: Some(now_ts),
        finished_at: None,
        duration_secs: None,
        updated_at: now_ts,
    })?;

    store.insert_session(&ticket_id, "ticket", None, now_ts)?;

    println!(
        "Ticket {} started at {}",
        ticket_id,
        now.format("%Y-%m-%d %H:%M UTC")
    );
    println!(
        "Reminder: Reference {} in all commits. Run 'ticket done {}' when complete.",
        ticket_id, ticket_id
    );

    Ok(())
}

pub fn start_batch(repo_root: &Path, batch_id: &str) -> Result<(), String> {
    let batch_id = batch_id.to_uppercase();

    let store = open_store_with_migration(repo_root)?;

    let active = store.active_sessions()?;
    if active
        .iter()
        .any(|s| s.ticket_id == batch_id && s.mode == "batch")
    {
        println!("Batch {} is already in progress", batch_id);
        return Ok(());
    }

    let backlog_path = Session::find_backlog_file_by_batch(repo_root, &batch_id)?;

    let now = Utc::now();
    let now_ts = now.timestamp();
    let backlog_str = backlog_path.to_string_lossy().into_owned();

    if store.get_ticket(&batch_id)?.is_none() {
        store.insert_ticket(&store::Ticket {
            ticket_id: batch_id.clone(),
            backlog: backlog_str,
            title: format!("Batch: {}", batch_id),
            spec_file: None,
            created_at: now_ts,
        })?;
    }

    store.insert_session(&batch_id, "batch", None, now_ts)?;

    println!(
        "Batch {} started at {}",
        batch_id,
        now.format("%Y-%m-%d %H:%M UTC")
    );
    println!("All tickets in {} are in scope", backlog_path.display());

    let all_active = store.active_sessions()?;
    let others: Vec<_> = all_active
        .iter()
        .filter(|s| s.ticket_id != batch_id)
        .collect();
    if !others.is_empty() {
        println!();
        println!("Other active sessions:");
        for s in &others {
            let start_str = chrono::DateTime::from_timestamp(s.started_at, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            println!("  {} ({}) — started {}", s.ticket_id, s.mode, start_str);
        }
    }

    Ok(())
}

pub fn done_batch(repo_root: &Path, batch_id: &str) -> Result<(), String> {
    let batch_id = batch_id.to_uppercase();

    let store = open_store_with_migration(repo_root)?;

    let active = store.active_sessions()?;
    let session = active
        .iter()
        .find(|s| s.ticket_id == batch_id && s.mode == "batch")
        .ok_or_else(|| {
            format!(
                "No active session for batch {}. Run 'ticket start --batch {}' first.",
                batch_id, batch_id
            )
        })?
        .clone();

    let end_time = Utc::now();
    let end_ts = end_time.timestamp();
    let duration = chrono::Duration::seconds(end_ts - session.started_at);

    store.close_session(session.session_id, end_ts)?;

    println!("Batch {} completed", batch_id);
    println!("  Duration: {}", format_duration(duration));
    println!("  End: {}", end_time.format("%Y-%m-%d %H:%M UTC"));

    Ok(())
}

pub fn done(repo_root: &Path, ticket_id: &str) -> Result<(), String> {
    let ticket_id = ticket_id.to_uppercase();

    let store = open_store_with_migration(repo_root)?;

    let active = store.active_sessions()?;
    let session = active
        .iter()
        .find(|s| s.ticket_id == ticket_id && s.mode != "batch")
        .ok_or_else(|| {
            format!(
                "No active session for ticket {}. Run 'ticket start {}' first.",
                ticket_id, ticket_id
            )
        })?
        .clone();

    let end_time = Utc::now();
    let end_ts = end_time.timestamp();
    let duration_secs = end_ts - session.started_at;
    let duration = chrono::Duration::seconds(duration_secs);

    store.close_session(session.session_id, end_ts)?;
    store.upsert_ticket_status(&store::TicketStatus {
        ticket_id: ticket_id.clone(),
        status: "done".to_string(),
        blocked_reason: None,
        started_at: Some(session.started_at),
        finished_at: Some(end_ts),
        duration_secs: Some(duration_secs),
        updated_at: end_ts,
    })?;

    println!("Ticket {} completed", ticket_id);
    println!("  Duration: {}", format_duration(duration));
    println!("  End: {}", end_time.format("%Y-%m-%d %H:%M UTC"));

    Ok(())
}

pub fn status(repo_root: &Path) -> Result<(), String> {
    let store = open_store_with_migration(repo_root)?;
    let report = reconcile_from_store(&store, repo_root)?;
    print!("{}", render_status(&report));
    Ok(())
}

pub fn reconcile(repo_root: &Path, json: bool, strict: bool) -> Result<(), String> {
    let store = open_store_with_migration(repo_root)?;
    let report = reconcile_from_store(&store, repo_root)?;

    if json {
        let out = serde_json::to_string_pretty(&report)
            .map_err(|e| format!("Failed to serialize reconcile report: {}", e))?;
        println!("{}", out);
    } else {
        print!("{}", render_reconcile(&report));
    }

    let success = if strict {
        report.sessions.iter().all(|s| {
            matches!(
                s.derived_status,
                DerivedStatus::Active | DerivedStatus::BatchActive
            )
        })
    } else {
        report.ok
    };

    if success {
        Ok(())
    } else {
        Err("Found stale or inconsistent sessions".to_string())
    }
}

pub fn blocked(repo_root: &Path, ticket_id: &str, reason: &str) -> Result<(), String> {
    let ticket_id = ticket_id.to_uppercase();

    let store = open_store_with_migration(repo_root)?;

    // Intentional: `blocked` is permitted without an active session so that a
    // ticket can be marked blocked before (or after) a session was started.
    // Any open sessions are closed as part of this command.
    let now_ts = Utc::now().timestamp();

    if store.get_ticket(&ticket_id)?.is_none() {
        let backlog_path = Session::find_backlog_file(repo_root, &ticket_id)?;
        let backlog_str = backlog_path.to_string_lossy().into_owned();
        store.insert_ticket(&store::Ticket {
            ticket_id: ticket_id.clone(),
            backlog: backlog_str,
            title: ticket_id.clone(),
            spec_file: None,
            created_at: now_ts,
        })?;
    }

    let started_at = store
        .get_ticket_status(&ticket_id)?
        .and_then(|s| s.started_at);

    store.upsert_ticket_status(&store::TicketStatus {
        ticket_id: ticket_id.clone(),
        status: "blocked".to_string(),
        blocked_reason: Some(reason.to_string()),
        started_at,
        finished_at: None,
        duration_secs: None,
        updated_at: now_ts,
    })?;

    let active = store.active_sessions()?;
    for session in active.iter().filter(|s| s.ticket_id == ticket_id) {
        store.close_session(session.session_id, now_ts)?;
    }

    store.add_note(&ticket_id, "blocker", reason, now_ts)?;

    println!("Ticket {} marked as Blocked", ticket_id);
    println!("Reason: {}", reason);

    Ok(())
}

pub fn note(repo_root: &Path, ticket_id: &str, note: &str) -> Result<(), String> {
    let ticket_id = ticket_id.to_uppercase();

    let store = open_store_with_migration(repo_root)?;

    let now_ts = Utc::now().timestamp();

    if store.get_ticket(&ticket_id)?.is_none() {
        let backlog_path = Session::find_backlog_file(repo_root, &ticket_id)?;
        let backlog_str = backlog_path.to_string_lossy().into_owned();
        store.insert_ticket(&store::Ticket {
            ticket_id: ticket_id.clone(),
            backlog: backlog_str,
            title: ticket_id.clone(),
            spec_file: None,
            created_at: now_ts,
        })?;
    }

    store.add_note(&ticket_id, "comment", note, now_ts)?;

    println!("Note added to ticket {}", ticket_id);

    Ok(())
}

// ── import helpers ────────────────────────────────────────────────────────────

/// Parse a timestamp string like `2026-03-29 03:29 UTC` or `2026-03-29 03:40:00 UTC`
/// into a Unix timestamp.  Returns `None` on any parse failure.
fn parse_timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    let base = s.trim_end_matches(" UTC");
    if let Ok(dt) = NaiveDateTime::parse_from_str(base, "%Y-%m-%d %H:%M") {
        return Some(dt.and_utc().timestamp());
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(base, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.and_utc().timestamp());
    }
    // ISO 8601
    if let Ok(dt) = s.parse::<chrono::DateTime<chrono::Utc>>() {
        return Some(dt.timestamp());
    }
    None
}

/// Parse a duration string like `00:10:50` into total seconds.
fn parse_duration_secs(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.trim().splitn(3, ':').collect();
    if parts.len() == 3 {
        let h: i64 = parts[0].parse().ok()?;
        let m: i64 = parts[1].parse().ok()?;
        let sec: i64 = parts[2].parse().ok()?;
        return Some(h * 3600 + m * 60 + sec);
    }
    None
}

/// Map backlog status strings to the store's canonical values.
fn map_status(s: &str) -> &str {
    match s.trim() {
        "Todo" => "todo",
        "In Progress" => "in_progress",
        "Done" => "done",
        "Blocked" => "blocked",
        other => other,
    }
}

// ── import ────────────────────────────────────────────────────────────────────

pub fn import(repo_root: &Path) -> Result<(), String> {
    let backlog_dir = repo_root.join(crate::BACKLOG_DIR);

    let pattern = backlog_dir.join("**/*.md").to_string_lossy().into_owned();

    let paths: Vec<std::path::PathBuf> = glob::glob(&pattern)
        .map_err(|e| format!("glob pattern error: {e}"))?
        .filter_map(|r| r.ok())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n != "template.md" && n != "NEXT.md" && n != "README.md")
                .unwrap_or(false)
        })
        .collect();

    let store = Store::open(repo_root)?;
    let now_ts = Utc::now().timestamp();

    let mut files_count: usize = 0;
    let mut tickets_new: usize = 0;
    let mut tickets_updated: usize = 0;
    let mut metrics_count: usize = 0;
    let mut notes_count: usize = 0;

    // Regexes compiled once.
    let ticket_heading_re =
        Regex::new(r"(?m)^(#{2,6}) Ticket ([A-Z][A-Z0-9]*-[A-Z0-9]+)(?:\s+[—\-]+\s+(.+))?$")
            .map_err(|e| format!("regex error: {e}"))?;
    let status_re =
        Regex::new(r"(?m)^- Status:\s*(.+)$").map_err(|e| format!("regex error: {e}"))?;
    let goal_re = Regex::new(r"(?m)^- Goal:\s*(.+)$").map_err(|e| format!("regex error: {e}"))?;

    // Metrics block regexes.
    let metrics_section_re =
        Regex::new(r"(?m)^## Execution Metrics\s*$").map_err(|e| format!("regex error: {e}"))?;
    let metrics_ticket_re = Regex::new(r"(?m)^- Ticket:\s*([A-Z][A-Z0-9]*-[A-Z0-9]+)\s*$")
        .map_err(|e| format!("regex error: {e}"))?;
    let metric_field_re = Regex::new(r"(?m)^- (Start|End|Duration):\s*(.+)$")
        .map_err(|e| format!("regex error: {e}"))?;

    // Tracking notes / closure evidence regexes.
    let tracking_section_re =
        Regex::new(r"(?m)^## Tracking Notes\s*$").map_err(|e| format!("regex error: {e}"))?;
    let note_line_re = Regex::new(r"(?m)^\s*-\s+\[([A-Z][A-Z0-9]*-[A-Z0-9]+)\]\s+(.+)$")
        .map_err(|e| format!("regex error: {e}"))?;
    let closure_section_re =
        Regex::new(r"(?m)^## Closure Evidence\s*$").map_err(|e| format!("regex error: {e}"))?;
    let next_h2_re = Regex::new(r"(?m)^## ").map_err(|e| format!("regex error: {e}"))?;

    for path in &paths {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: skipping {}: {e}", path.display());
                continue;
            }
        };
        files_count += 1;

        let backlog_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let spec_file = path
            .strip_prefix(repo_root)
            .ok()
            .map(|p| p.to_string_lossy().into_owned());

        // ── tickets ──────────────────────────────────────────────────────────

        // Collect all ticket heading positions so we can bound each section.
        let heading_matches: Vec<_> = ticket_heading_re.captures_iter(&content).collect();

        for (i, caps) in heading_matches.iter().enumerate() {
            let full_match = caps.get(0).unwrap();
            let heading_hashes = caps.get(1).unwrap().as_str();
            let ticket_id = caps.get(2).unwrap().as_str().to_string();
            let title_from_heading = caps
                .get(3)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();

            // Section ends at the next same-or-higher-level heading, or EOF.
            let section_start = full_match.start();
            let section_end = {
                let depth = heading_hashes.len();
                let same_level_pat = format!(r"(?m)^#{{2,{depth}}}\s");
                let next_re = Regex::new(&same_level_pat)
                    .map_err(|e| format!("regex error building section boundary: {e}"))?;
                let search_from = full_match.end();
                let next_ticket_pos = heading_matches
                    .get(i + 1)
                    .map(|c| c.get(0).unwrap().start());
                let next_section_pos = next_re.find_at(&content, search_from).map(|m| m.start());
                match (next_ticket_pos, next_section_pos) {
                    (Some(a), Some(b)) => a.min(b),
                    (Some(a), None) => a,
                    (None, Some(b)) => b,
                    (None, None) => content.len(),
                }
            };

            let section = &content[section_start..section_end];

            let status_str = status_re
                .captures(section)
                .map(|c| c[1].trim().to_string())
                .unwrap_or_default();

            let title = if !title_from_heading.is_empty() {
                title_from_heading
            } else {
                goal_re
                    .captures(section)
                    .map(|c| c[1].trim().to_string())
                    .unwrap_or_else(|| ticket_id.clone())
            };

            let ticket = Ticket {
                ticket_id: ticket_id.clone(),
                backlog: backlog_id.clone(),
                title,
                spec_file: spec_file.clone(),
                created_at: now_ts,
            };

            match store.upsert_ticket(&ticket) {
                Ok(true) => tickets_new += 1,
                Ok(false) => tickets_updated += 1,
                Err(e) => {
                    eprintln!("warning: skipping ticket {ticket_id}: {e}");
                    continue;
                }
            }

            if !status_str.is_empty() {
                let ts = TicketStatus {
                    ticket_id: ticket_id.clone(),
                    status: map_status(&status_str).to_string(),
                    blocked_reason: None,
                    started_at: None,
                    finished_at: None,
                    duration_secs: None,
                    updated_at: now_ts,
                };
                if let Err(e) = store.upsert_ticket_status(&ts) {
                    eprintln!("warning: status upsert for {ticket_id}: {e}");
                }
            }
        }

        // ── execution metrics ─────────────────────────────────────────────────

        if let Some(ms) = metrics_section_re.find(&content) {
            let metrics_start = ms.end();
            let metrics_end = next_h2_re
                .find_at(&content, metrics_start)
                .map(|m| m.start())
                .unwrap_or(content.len());
            let metrics_block = &content[metrics_start..metrics_end];

            let entry_starts: Vec<_> = metrics_ticket_re
                .captures_iter(metrics_block)
                .map(|c| {
                    let tid = c[1].to_string();
                    let pos = c.get(0).unwrap().end();
                    (tid, pos)
                })
                .collect();

            for (j, (tid, entry_body_start)) in entry_starts.iter().enumerate() {
                let entry_end = entry_starts
                    .get(j + 1)
                    .map(|(next_tid, p)| *p - next_tid.len() - "- Ticket: ".len())
                    .unwrap_or(metrics_block.len());
                let entry_body = &metrics_block[*entry_body_start..entry_end];

                // Only update if ticket is known in the store.
                if store.get_ticket(tid)?.is_none() {
                    continue;
                }

                let mut started_at: Option<i64> = None;
                let mut finished_at: Option<i64> = None;
                let mut duration_secs: Option<i64> = None;

                for fc in metric_field_re.captures_iter(entry_body) {
                    let field = &fc[1];
                    let val = fc[2].trim();
                    match field {
                        "Start" => started_at = parse_timestamp(val),
                        "End" => finished_at = parse_timestamp(val),
                        "Duration" => duration_secs = parse_duration_secs(val),
                        _ => {}
                    }
                }

                if started_at.is_some() || finished_at.is_some() || duration_secs.is_some() {
                    // Merge with any existing status row.
                    let existing = store.get_ticket_status(tid)?;
                    let base_status = existing
                        .as_ref()
                        .map(|s| s.status.clone())
                        .unwrap_or_else(|| "todo".to_string());
                    let ts = TicketStatus {
                        ticket_id: tid.clone(),
                        status: base_status,
                        blocked_reason: existing.as_ref().and_then(|s| s.blocked_reason.clone()),
                        started_at: started_at.or(existing.as_ref().and_then(|s| s.started_at)),
                        finished_at: finished_at.or(existing.as_ref().and_then(|s| s.finished_at)),
                        duration_secs: duration_secs
                            .or(existing.as_ref().and_then(|s| s.duration_secs)),
                        updated_at: now_ts,
                    };
                    if let Err(e) = store.upsert_ticket_status(&ts) {
                        eprintln!("warning: metrics upsert for {tid}: {e}");
                    } else {
                        metrics_count += 1;
                    }
                }
            }
        }

        // ── tracking notes ────────────────────────────────────────────────────

        if let Some(ts_match) = tracking_section_re.find(&content) {
            let notes_start = ts_match.end();
            let notes_end = next_h2_re
                .find_at(&content, notes_start)
                .map(|m| m.start())
                .unwrap_or(content.len());
            let notes_block = &content[notes_start..notes_end];

            for nc in note_line_re.captures_iter(notes_block) {
                let tid = &nc[1];
                let body = nc[2].trim();
                if store.get_ticket(tid)?.is_none() {
                    continue;
                }
                if !store.note_exists(tid, "note", body)? {
                    if let Err(e) = store.add_note(tid, "note", body, now_ts) {
                        eprintln!("warning: add note for {tid}: {e}");
                    } else {
                        notes_count += 1;
                    }
                }
            }
        }

        // ── closure evidence ──────────────────────────────────────────────────

        if let Some(ce_match) = closure_section_re.find(&content) {
            let ce_start = ce_match.end();
            let ce_end = next_h2_re
                .find_at(&content, ce_start)
                .map(|m| m.start())
                .unwrap_or(content.len());
            let ce_body = content[ce_start..ce_end].trim();

            if !ce_body.is_empty() {
                // We store this as a note on any ticket in the file; pick the
                // first one we can find that actually exists in the store.
                let first_tid = ticket_heading_re
                    .captures_iter(&content)
                    .map(|c| c[2].to_string())
                    .find(|tid| store.get_ticket(tid).ok().flatten().is_some());

                if let Some(tid) = first_tid {
                    if !store.note_exists(&tid, "closure_evidence", ce_body)? {
                        if let Err(e) = store.add_note(&tid, "closure_evidence", ce_body, now_ts) {
                            eprintln!("warning: closure evidence for {tid}: {e}");
                        } else {
                            notes_count += 1;
                        }
                    }
                } else {
                    // No ticket in this file was imported — cannot attach the note.
                    // Warn so the operator knows the evidence was skipped.
                    eprintln!(
                        "warning: closure evidence in {} skipped (no imported ticket found)",
                        path.display()
                    );
                }
            }
        }
    }

    let tickets_total = tickets_new + tickets_updated;
    println!("Imported from {files_count} backlog files:");
    println!("  Tickets: {tickets_total} ({tickets_new} new, {tickets_updated} updated)");
    println!("  Metrics: {metrics_count} entries");
    println!("  Notes: {notes_count} entries");

    Ok(())
}

pub fn report(
    repo_root: &Path,
    ticket_id: Option<&str>,
    batch: Option<&str>,
) -> Result<(), String> {
    let store = open_store_with_migration(repo_root)?;

    match (ticket_id, batch) {
        (Some(id), None) => {
            let id = id.to_uppercase();
            print!("{}", render_ticket_report(&store, &id)?);
            Ok(())
        }
        (None, Some(batch_id)) => {
            let batch_id = batch_id.to_uppercase();
            let tickets = store.list_tickets()?;
            let prefix = format!("{}-", batch_id);
            let matching: Vec<_> = tickets
                .iter()
                .filter(|t| t.ticket_id.starts_with(&prefix) || t.ticket_id == batch_id)
                .collect();
            if matching.is_empty() {
                println!("No tickets found for batch {}", batch_id);
                return Ok(());
            }
            for ticket in &matching {
                print!("{}", render_ticket_report(&store, &ticket.ticket_id)?);
                println!();
            }
            Ok(())
        }
        (Some(_), Some(_)) => Err("Specify either a ticket ID or --batch, not both".to_string()),
        (None, None) => Err("Specify a ticket ID or --batch <batch-id>".to_string()),
    }
}

fn render_ticket_report(store: &crate::store::Store, ticket_id: &str) -> Result<String, String> {
    let tws = store.ticket_with_status(ticket_id)?.ok_or_else(|| {
        format!(
            "Ticket {} not found in store. Run 'ticket import' first.",
            ticket_id
        )
    })?;

    let mut out = String::new();

    out.push_str(&format!("Ticket: {}\n", tws.ticket.ticket_id));

    if let Some(ref ts) = tws.status {
        out.push_str(&format!("Status: {}\n", ts.status));

        let fmt_ts = |epoch: i64| {
            chrono::DateTime::from_timestamp(epoch, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "unknown".to_string())
        };

        if let Some(started) = ts.started_at {
            out.push_str(&format!("Started: {}\n", fmt_ts(started)));
        }
        if let Some(finished) = ts.finished_at {
            out.push_str(&format!("Finished: {}\n", fmt_ts(finished)));
        }
        if let Some(secs) = ts.duration_secs {
            let dur = chrono::Duration::seconds(secs);
            out.push_str(&format!("Duration: {}\n", format_duration(dur)));
        }
        if let Some(ref reason) = ts.blocked_reason {
            out.push_str(&format!("Blocked reason: {}\n", reason));
        }
    } else {
        out.push_str("Status: not started\n");
    }

    let notes = store.list_notes(ticket_id)?;
    // "comment" = added by `ticket note`; "note" = imported from Tracking Notes
    let comments: Vec<_> = notes
        .iter()
        .filter(|n| n.kind == "comment" || n.kind == "note")
        .collect();
    let closure: Vec<_> = notes
        .iter()
        .filter(|n| n.kind == "closure_evidence")
        .collect();

    out.push_str(&format!("\nNotes ({}):\n", comments.len()));
    if comments.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for note in &comments {
            let ts_str = chrono::DateTime::from_timestamp(note.created_at, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            out.push_str(&format!("  [{}] {}\n", ts_str, note.body));
        }
    }

    out.push_str("\nClosure Evidence:\n");
    if closure.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for note in &closure {
            let ts_str = chrono::DateTime::from_timestamp(note.created_at, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            out.push_str(&format!("  [{}] {}\n", ts_str, note.body));
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReconcileSummary, SessionDiagnostic, BACKLOG_DIR};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ── render helpers ────────────────────────────────────────────────────────

    fn report_with_session(session: SessionDiagnostic) -> ReconcileReport {
        ReconcileReport {
            ok: matches!(
                session.derived_status,
                DerivedStatus::Active | DerivedStatus::BatchActive
            ),
            summary: ReconcileSummary {
                total_sessions: 1,
                active: usize::from(matches!(session.derived_status, DerivedStatus::Active)),
                batch_active: usize::from(matches!(
                    session.derived_status,
                    DerivedStatus::BatchActive
                )),
                stale_done: usize::from(matches!(session.derived_status, DerivedStatus::StaleDone)),
                stale_blocked: usize::from(matches!(
                    session.derived_status,
                    DerivedStatus::StaleBlocked
                )),
                status_mismatch: usize::from(matches!(
                    session.derived_status,
                    DerivedStatus::StatusMismatch
                )),
                missing_backlog: usize::from(matches!(
                    session.derived_status,
                    DerivedStatus::MissingBacklog
                )),
                missing_ticket: usize::from(matches!(
                    session.derived_status,
                    DerivedStatus::MissingTicket
                )),
                invalid_session: usize::from(matches!(
                    session.derived_status,
                    DerivedStatus::InvalidSession
                )),
            },
            sessions: vec![session],
        }
    }

    // ── render tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_render_status_reports_stale_done() {
        let output = render_status(&report_with_session(SessionDiagnostic {
            ticket_id: "TEST-1".to_string(),
            mode: "ticket".to_string(),
            file: PathBuf::from("docs/project/backlog/test.md"),
            start: None,
            derived_status: DerivedStatus::StaleDone,
            backlog_status: Some("Done".to_string()),
            message: Some("Session is still open but ticket is Done in backlog".to_string()),
        }));

        assert!(output.contains("Status: Stale (Done in backlog)"));
        assert!(output.contains("Backlog Status: Done"));
    }

    #[test]
    fn test_render_reconcile_reports_consistent_sessions() {
        let output = render_reconcile(&report_with_session(SessionDiagnostic {
            ticket_id: "BATCH".to_string(),
            mode: "batch".to_string(),
            file: PathBuf::from("docs/project/backlog/test.md"),
            start: None,
            derived_status: DerivedStatus::BatchActive,
            backlog_status: None,
            message: None,
        }));

        assert!(output.contains("All sessions are consistent."));
    }

    #[test]
    fn test_render_reconcile_reports_problem_details() {
        let output = render_reconcile(&report_with_session(SessionDiagnostic {
            ticket_id: "TEST-1".to_string(),
            mode: "ticket".to_string(),
            file: PathBuf::from("docs/project/backlog/test.md"),
            start: None,
            derived_status: DerivedStatus::MissingTicket,
            backlog_status: None,
            message: Some("Ticket TEST-1 not found".to_string()),
        }));

        assert!(output.contains("Problems"));
        assert!(output.contains("TEST-1 (Invalid (Missing ticket heading))"));
        assert!(output.contains("Message: Ticket TEST-1 not found"));
    }

    // ── import ────────────────────────────────────────────────────────────────

    fn setup_import_repo(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let backlog_dir = dir.path().join("docs").join("project").join("backlog");
        fs::create_dir_all(&backlog_dir).expect("create backlog dir");
        let md_path = backlog_dir.join("project.md");
        fs::write(&md_path, content).expect("write md");
        (dir, md_path)
    }

    const SAMPLE_MD: &str = "\
## Ticket IMP-1 — First ticket
- Goal: Build the thing
- In scope:
  - core
- Out of scope:
  - docs
- Dependencies: none
- Acceptance criteria:
  - tests pass
- Verification:
  - cargo test
- Status: Done

## Ticket IMP-2 — Second ticket
- Goal: Extend the thing
- In scope:
  - api
- Out of scope:
  - ui
- Dependencies: IMP-1
- Acceptance criteria:
  - api works
- Verification:
  - cargo test
- Status: In Progress

## Tracking Notes

- [IMP-1] Initial implementation complete

## Execution Metrics

- Ticket: IMP-1
- Start: 2026-01-01 10:00 UTC
- End: 2026-01-01 10:30 UTC
- Duration: 00:30:00

- Ticket: IMP-2
- Start: 2026-01-02 09:00 UTC

## Closure Evidence

PR #42 merged and deployed.
";

    #[test]
    fn test_import_populates_tickets_and_status() {
        let (dir, _) = setup_import_repo(SAMPLE_MD);
        import(dir.path()).expect("import");

        let store = Store::open(dir.path()).expect("open store");

        let t1 = store.get_ticket("IMP-1").expect("get").expect("some");
        assert_eq!(t1.backlog, "project");
        assert_eq!(t1.title, "First ticket");

        let s1 = store
            .get_ticket_status("IMP-1")
            .expect("status")
            .expect("some");
        assert_eq!(s1.status, "done");
        assert_eq!(s1.started_at, Some(1_767_261_600)); // 2026-01-01 10:00 UTC
        assert_eq!(s1.finished_at, Some(1_767_263_400)); // 2026-01-01 10:30 UTC
        assert_eq!(s1.duration_secs, Some(1_800));

        let s2 = store
            .get_ticket_status("IMP-2")
            .expect("status")
            .expect("some");
        assert_eq!(s2.status, "in_progress");
        assert_eq!(s2.started_at, Some(1_767_344_400)); // 2026-01-02 09:00 UTC
    }

    #[test]
    fn test_import_idempotent() {
        let (dir, _) = setup_import_repo(SAMPLE_MD);
        import(dir.path()).expect("first import");
        import(dir.path()).expect("second import — must not fail or duplicate");

        let store = Store::open(dir.path()).expect("open store");
        let tickets = store.list_tickets().expect("list");
        assert_eq!(tickets.len(), 2, "no duplicate tickets on re-import");

        let notes = store.list_notes("IMP-1").expect("notes");
        // 1 tracking note + 1 closure evidence
        assert_eq!(notes.len(), 2, "no duplicate notes on re-import");
    }

    #[test]
    fn test_import_tracking_note_stored() {
        let (dir, _) = setup_import_repo(SAMPLE_MD);
        import(dir.path()).expect("import");

        let store = Store::open(dir.path()).expect("open store");
        let notes = store.list_notes("IMP-1").expect("notes");
        let note = notes.iter().find(|n| n.kind == "note").expect("note");
        assert_eq!(note.body, "Initial implementation complete");
    }

    #[test]
    fn test_import_closure_evidence_stored() {
        let (dir, _) = setup_import_repo(SAMPLE_MD);
        import(dir.path()).expect("import");

        let store = Store::open(dir.path()).expect("open store");
        let notes = store.list_notes("IMP-1").expect("notes");
        let ce = notes
            .iter()
            .find(|n| n.kind == "closure_evidence")
            .expect("closure evidence");
        assert!(ce.body.contains("PR #42"));
    }

    #[test]
    fn test_import_skips_template_and_next() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let backlog_dir = dir.path().join("docs").join("project").join("backlog");
        fs::create_dir_all(&backlog_dir).expect("create backlog dir");
        fs::write(
            backlog_dir.join("template.md"),
            "## Ticket TMPL-1\n- Status: Todo\n",
        )
        .expect("write template");
        fs::write(
            backlog_dir.join("NEXT.md"),
            "## Ticket NEXT-1\n- Status: Todo\n",
        )
        .expect("write NEXT");

        import(dir.path()).expect("import");

        let store = Store::open(dir.path()).expect("open store");
        assert!(store.list_tickets().expect("list").is_empty());
    }

    #[test]
    fn test_import_recurses_subdirectories() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let sub_dir = dir
            .path()
            .join("docs")
            .join("project")
            .join("backlog")
            .join("compiler");
        fs::create_dir_all(&sub_dir).expect("create subdirectory");
        fs::write(
            sub_dir.join("batch1.md"),
            "## Ticket SUB-1 — Nested ticket\n\
             - Goal: Test recursive import\n\
             - In scope:\n  - x\n\
             - Out of scope:\n  - y\n\
             - Dependencies: none\n\
             - Acceptance criteria:\n  - pass\n\
             - Verification:\n  - cargo test\n\
             - Status: Done\n",
        )
        .expect("write nested backlog");

        import(dir.path()).expect("import");

        let store = Store::open(dir.path()).expect("open store");
        let tickets = store.list_tickets().expect("list");
        assert_eq!(tickets.len(), 1);
        assert_eq!(tickets[0].ticket_id, "SUB-1");
    }

    // ── command integration tests ─────────────────────────────────────────────

    fn make_backlog_ticket(repo: &TempDir, ticket_id: &str, status: &str) -> PathBuf {
        let backlog_dir = repo.path().join(BACKLOG_DIR);
        fs::create_dir_all(&backlog_dir).expect("create backlog dir");
        let path = backlog_dir.join("test.md");
        let content = format!(
            "## Ticket {} — Test\n\
             - Goal: Test goal\n\
             - In scope:\n  - x\n\
             - Out of scope:\n  - y\n\
             - Dependencies: none\n\
             - Acceptance criteria:\n  - pass\n\
             - Verification:\n  - cargo test\n\
             - Status: {}\n",
            ticket_id, status
        );
        fs::write(&path, content).expect("write backlog");
        path
    }

    #[test]
    fn test_start_creates_sqlite_session() {
        let repo = TempDir::new().expect("tempdir");
        make_backlog_ticket(&repo, "TEST-1", "Todo");
        start(repo.path(), "TEST-1").expect("start");

        let store = store::Store::open(repo.path()).expect("open store");
        let sessions = store.active_sessions().expect("active sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].ticket_id, "TEST-1");
        assert_eq!(sessions[0].mode, "ticket");
    }

    #[test]
    fn test_start_idempotent() {
        let repo = TempDir::new().expect("tempdir");
        make_backlog_ticket(&repo, "TEST-1", "Todo");
        start(repo.path(), "TEST-1").expect("first start");
        start(repo.path(), "TEST-1").expect("second start");

        let store = store::Store::open(repo.path()).expect("open store");
        assert_eq!(store.active_sessions().expect("sessions").len(), 1);
    }

    #[test]
    fn test_start_rejects_done_ticket() {
        let repo = TempDir::new().expect("tempdir");
        make_backlog_ticket(&repo, "TEST-1", "Done");
        let err = start(repo.path(), "TEST-1").expect_err("should error");
        assert!(err.contains("already Done"));
    }

    #[test]
    fn test_start_rejects_sqlite_done_ticket() {
        // Ensure the SQLite done-guard fires even when .md still says "Todo".
        let repo = TempDir::new().expect("tempdir");
        make_backlog_ticket(&repo, "TEST-1", "Todo");
        start(repo.path(), "TEST-1").expect("start");
        done(repo.path(), "TEST-1").expect("done");
        // .md still says "Todo", but SQLite records status = "done".
        let err = start(repo.path(), "TEST-1").expect_err("second start should be rejected");
        assert!(err.contains("already Done"));
    }

    #[test]
    fn test_start_does_not_modify_backlog_md() {
        let repo = TempDir::new().expect("tempdir");
        let path = make_backlog_ticket(&repo, "TEST-1", "Todo");
        let before = fs::read_to_string(&path).expect("read");
        start(repo.path(), "TEST-1").expect("start");
        let after = fs::read_to_string(&path).expect("read after");
        assert_eq!(before, after, "start must not modify the backlog .md");
    }

    #[test]
    fn test_done_closes_session_and_updates_status() {
        let repo = TempDir::new().expect("tempdir");
        make_backlog_ticket(&repo, "TEST-1", "Todo");
        start(repo.path(), "TEST-1").expect("start");
        done(repo.path(), "TEST-1").expect("done");

        let store = store::Store::open(repo.path()).expect("open store");
        assert!(store.active_sessions().expect("sessions").is_empty());
        let ts = store
            .get_ticket_status("TEST-1")
            .expect("status")
            .expect("some");
        assert_eq!(ts.status, "done");
        assert!(ts.finished_at.is_some());
        assert!(ts.duration_secs.is_some());
    }

    #[test]
    fn test_done_does_not_modify_backlog_md() {
        let repo = TempDir::new().expect("tempdir");
        let path = make_backlog_ticket(&repo, "TEST-1", "Todo");
        start(repo.path(), "TEST-1").expect("start");
        let before = fs::read_to_string(&path).expect("read");
        done(repo.path(), "TEST-1").expect("done");
        let after = fs::read_to_string(&path).expect("read after");
        assert_eq!(before, after, "done must not modify the backlog .md");
    }

    #[test]
    fn test_done_without_session_errors() {
        let repo = TempDir::new().expect("tempdir");
        let err = done(repo.path(), "TEST-1").expect_err("should error");
        assert!(err.contains("No active session"));
    }

    #[test]
    fn test_blocked_closes_session_and_updates_status() {
        let repo = TempDir::new().expect("tempdir");
        make_backlog_ticket(&repo, "TEST-1", "Todo");
        start(repo.path(), "TEST-1").expect("start");
        blocked(repo.path(), "TEST-1", "waiting for dep").expect("blocked");

        let store = store::Store::open(repo.path()).expect("open store");
        assert!(store.active_sessions().expect("sessions").is_empty());
        let ts = store
            .get_ticket_status("TEST-1")
            .expect("status")
            .expect("some");
        assert_eq!(ts.status, "blocked");
        assert_eq!(ts.blocked_reason.as_deref(), Some("waiting for dep"));
        let notes = store.list_notes("TEST-1").expect("notes");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, "blocker");
        assert_eq!(notes[0].body, "waiting for dep");
    }

    #[test]
    fn test_note_adds_comment_to_store() {
        let repo = TempDir::new().expect("tempdir");
        make_backlog_ticket(&repo, "TEST-1", "Todo");
        start(repo.path(), "TEST-1").expect("start");
        note(repo.path(), "TEST-1", "edge case found").expect("note");

        let store = store::Store::open(repo.path()).expect("open store");
        let notes = store.list_notes("TEST-1").expect("notes");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, "comment");
        assert_eq!(notes[0].body, "edge case found");
    }

    #[test]
    fn test_reconcile_active_session_is_ok() {
        let repo = TempDir::new().expect("tempdir");
        make_backlog_ticket(&repo, "TEST-1", "Todo");
        start(repo.path(), "TEST-1").expect("start");
        reconcile(repo.path(), false, false).expect("reconcile must succeed");
    }

    #[test]
    fn test_reconcile_strict_passes_on_batch_session() {
        let repo = TempDir::new().expect("tempdir");
        let backlog_dir = repo.path().join(BACKLOG_DIR);
        fs::create_dir_all(&backlog_dir).expect("create backlog dir");
        fs::write(
            backlog_dir.join("batch.md"),
            "## Ticket BATCH-1 — T\n- Status: Todo\n",
        )
        .expect("write");
        start_batch(repo.path(), "BATCH").expect("start batch");

        reconcile(repo.path(), false, true).expect("strict should pass for batch sessions");
    }

    #[test]
    fn test_reconcile_missing_ticket_heading() {
        let repo = TempDir::new().expect("tempdir");
        make_backlog_ticket(&repo, "TEST-1", "Todo");
        start(repo.path(), "TEST-1").expect("start");

        // Remove the ticket heading from backlog so reconcile detects it missing
        let backlog_path = repo.path().join(BACKLOG_DIR).join("test.md");
        fs::write(&backlog_path, "## Ticket OTHER-1 — T\n- Status: Todo\n").expect("overwrite");

        let err = reconcile(repo.path(), false, false).expect_err("should detect missing ticket");
        assert!(err.contains("stale or inconsistent"));
    }

    #[test]
    fn test_yaml_migration_on_first_command() {
        let repo = TempDir::new().expect("tempdir");
        let path = make_backlog_ticket(&repo, "TEST-1", "Todo");

        // Write a legacy YAML session
        let sessions_dir = repo.path().join(SESSION_DIR);
        fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        let yaml = format!(
            "ticket_id: TEST-1\nstart: 2026-01-01T00:00:00Z\nfile: {}\nmode: ticket\n",
            path.display()
        );
        fs::write(sessions_dir.join("TEST-1.yaml"), &yaml).expect("write yaml");

        // First command triggers migration; the YAML session is already active
        start(repo.path(), "TEST-1").expect("start (idempotent after migration)");

        let store = store::Store::open(repo.path()).expect("open store");
        let sessions = store.active_sessions().expect("sessions");
        assert!(!sessions.is_empty());
        assert_eq!(sessions[0].ticket_id, "TEST-1");
    }

    // ── report ────────────────────────────────────────────────────────────────

    #[test]
    fn test_report_ticket_not_in_store() {
        let repo = TempDir::new().expect("tempdir");
        let err = report(repo.path(), Some("MISSING-1"), None).expect_err("should error");
        assert!(err.contains("MISSING-1"));
        assert!(err.contains("not found in store"));
    }

    #[test]
    fn test_report_not_started() {
        let repo = TempDir::new().expect("tempdir");
        // Insert a ticket with no status row.
        let store = store::Store::open(repo.path()).expect("open store");
        store
            .insert_ticket(&store::Ticket {
                ticket_id: "NS-1".to_string(),
                backlog: "project".to_string(),
                title: "Not started".to_string(),
                spec_file: None,
                created_at: 1_000_000,
            })
            .expect("insert");
        drop(store);

        report(repo.path(), Some("NS-1"), None).expect("report");
        // No panic; spot-check via render path indirectly through render_ticket_report.
        let store2 = store::Store::open(repo.path()).expect("open store 2");
        let out = render_ticket_report(&store2, "NS-1").expect("render");
        assert!(out.contains("Ticket: NS-1"));
        assert!(out.contains("Status: not started"));
        assert!(out.contains("Notes (0):"));
        assert!(out.contains("Closure Evidence:"));
    }

    #[test]
    fn test_report_done_with_notes() {
        let repo = TempDir::new().expect("tempdir");
        make_backlog_ticket(&repo, "RPT-1", "Todo");
        start(repo.path(), "RPT-1").expect("start");
        note(repo.path(), "RPT-1", "edge case found").expect("note");
        done(repo.path(), "RPT-1").expect("done");

        // Add a closure_evidence note directly.
        let store = store::Store::open(repo.path()).expect("open store");
        store
            .add_note("RPT-1", "closure_evidence", "PR #99 merged", 9_000_000)
            .expect("add ce note");

        let out = render_ticket_report(&store, "RPT-1").expect("render");
        assert!(out.contains("Ticket: RPT-1"));
        assert!(out.contains("Status: done"));
        assert!(out.contains("Started:"));
        assert!(out.contains("Finished:"));
        assert!(out.contains("Duration:"));
        assert!(out.contains("Notes (1):"));
        assert!(out.contains("edge case found"));
        assert!(out.contains("Closure Evidence:"));
        assert!(out.contains("PR #99 merged"));
    }

    #[test]
    fn test_report_batch_lists_all_tickets() {
        let repo = TempDir::new().expect("tempdir");
        let store = store::Store::open(repo.path()).expect("open store");
        for id in ["BATCH-1", "BATCH-2", "BATCH-3"] {
            store
                .insert_ticket(&store::Ticket {
                    ticket_id: id.to_string(),
                    backlog: "batch".to_string(),
                    title: id.to_string(),
                    spec_file: None,
                    created_at: 1_000_000,
                })
                .expect("insert");
        }
        // BATCH-99 uses the same prefix but should not appear under --batch BATCH
        // because it doesn't start with "BATCH-" in a meaningful way (it does, actually —
        // the prefix filter is "BATCH-", so BATCH-99 would match).
        drop(store);

        // report --batch BATCH should list all three tickets without error.
        report(repo.path(), None, Some("BATCH")).expect("batch report");
    }
}
