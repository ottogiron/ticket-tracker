use crate::store::{Store, Ticket, TicketStatus};
use crate::{Backlog, DerivedStatus, ReconcileReport, Session};
use chrono::{NaiveDateTime, Utc};
use regex::Regex;
use std::fs;
use std::path::Path;

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

pub fn start(repo_root: &Path, ticket_id: &str) -> Result<(), String> {
    let ticket_id = ticket_id.to_uppercase();

    if let Some(session) = Session::read(repo_root, &ticket_id)? {
        if session.ticket_id == ticket_id {
            println!("Ticket {} is already in progress", ticket_id);
            return Ok(());
        }
    }

    let backlog_path = Session::find_backlog_file(repo_root, &ticket_id)?;
    let mut backlog = Backlog::read(&backlog_path)?;
    backlog.validate_required_ticket_schema(&ticket_id)?;

    let current_status = backlog.get_status(&ticket_id)?;
    if current_status == "Done" {
        return Err(format!(
            "Ticket {} is already Done. Create a new ticket or re-open this one.",
            ticket_id
        ));
    }
    if current_status == "In Progress" {
        println!("Resuming ticket {} (session was lost)", ticket_id);
    }

    backlog.update_status(&ticket_id, "In Progress")?;
    backlog.ensure_metrics_entry(&ticket_id)?;
    let now = Utc::now();
    backlog.set_start_time(&ticket_id, now)?;
    backlog.write()?;

    let session = Session::new(&ticket_id, backlog_path);
    session.write(repo_root)?;

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

    if let Some(session) = Session::read(repo_root, &batch_id)? {
        if session.ticket_id == batch_id && session.mode == "batch" {
            println!("Batch {} is already in progress", batch_id);
            return Ok(());
        }
    }

    let backlog_path = Session::find_backlog_file_by_batch(repo_root, &batch_id)?;

    let now = Utc::now();
    let session = Session::new_batch(&batch_id, backlog_path.clone());
    session.write(repo_root)?;

    println!(
        "Batch {} started at {}",
        batch_id,
        now.format("%Y-%m-%d %H:%M UTC")
    );
    println!("All tickets in {} are in scope", backlog_path.display());

    // Show other active sessions if any
    let all = Session::list_all(repo_root)?;
    let others: Vec<_> = all.iter().filter(|s| s.ticket_id != batch_id).collect();
    if !others.is_empty() {
        println!();
        println!("Other active sessions:");
        for s in &others {
            println!(
                "  {} ({}) — started {}",
                s.ticket_id,
                s.mode,
                s.start.format("%Y-%m-%d %H:%M UTC")
            );
        }
    }

    Ok(())
}

pub fn done_batch(repo_root: &Path, batch_id: &str) -> Result<(), String> {
    let batch_id = batch_id.to_uppercase();

    let session = Session::read(repo_root, &batch_id)?.ok_or_else(|| {
        format!(
            "No active session for batch {}. Run 'ticket start --batch {}' first.",
            batch_id, batch_id
        )
    })?;

    if session.mode != "batch" {
        return Err(format!(
            "Session {} is ticket-mode, not batch-mode. Use 'ticket done {}' instead.",
            session.ticket_id, session.ticket_id
        ));
    }

    let duration = session.elapsed();
    let end_time = Utc::now();

    Session::remove(repo_root, &batch_id)?;

    println!("Batch {} completed", batch_id);
    println!("  Duration: {}", format_duration(duration));
    println!("  End: {}", end_time.format("%Y-%m-%d %H:%M UTC"));

    Ok(())
}

pub fn done(repo_root: &Path, ticket_id: &str) -> Result<(), String> {
    let ticket_id = ticket_id.to_uppercase();

    let session = Session::read(repo_root, &ticket_id)?.ok_or_else(|| {
        format!(
            "No active session for ticket {}. Run 'ticket start {}' first.",
            ticket_id, ticket_id
        )
    })?;

    let mut backlog = Backlog::read(&session.file)?;

    let end_time = Utc::now();
    let duration = session.elapsed();

    backlog.update_status(&ticket_id, "Done")?;
    backlog.ensure_metrics_entry(&ticket_id)?;
    backlog.set_end_time(&ticket_id, end_time)?;
    backlog.set_duration(&ticket_id, &format_duration(duration))?;
    backlog.write()?;

    Session::remove(repo_root, &ticket_id)?;

    println!("Ticket {} completed", ticket_id);
    println!("  Duration: {}", format_duration(duration));
    println!("  End: {}", end_time.format("%Y-%m-%d %H:%M UTC"));

    Ok(())
}

pub fn status(repo_root: &Path) -> Result<(), String> {
    let report = Session::reconcile_all(repo_root)?;
    print!("{}", render_status(&report));
    Ok(())
}

pub fn reconcile(repo_root: &Path, json: bool) -> Result<(), String> {
    let report = Session::reconcile_all(repo_root)?;

    if json {
        let json = serde_json::to_string_pretty(&report)
            .map_err(|e| format!("Failed to serialize reconcile report: {}", e))?;
        println!("{}", json);
    } else {
        print!("{}", render_reconcile(&report));
    }

    if report.ok {
        Ok(())
    } else {
        Err("Found stale or inconsistent sessions".to_string())
    }
}

pub fn blocked(repo_root: &Path, ticket_id: &str, reason: &str) -> Result<(), String> {
    let ticket_id = ticket_id.to_uppercase();

    let session = Session::read(repo_root, &ticket_id)?;
    let backlog_path = if let Some(ref session) = session {
        session.file.clone()
    } else {
        Session::find_backlog_file(repo_root, &ticket_id)?
    };

    let mut backlog = Backlog::read(&backlog_path)?;
    backlog.update_status(&ticket_id, "Blocked")?;
    backlog.add_tracking_note(&ticket_id, &format!("Blocked: {}", reason))?;
    backlog.write()?;

    Session::remove(repo_root, &ticket_id)?;

    println!("Ticket {} marked as Blocked", ticket_id);
    println!("Reason: {}", reason);

    Ok(())
}

pub fn note(repo_root: &Path, ticket_id: &str, note: &str) -> Result<(), String> {
    let ticket_id = ticket_id.to_uppercase();

    let backlog_path = Session::find_backlog_file(repo_root, &ticket_id)?;
    let mut backlog = Backlog::read(&backlog_path)?;
    backlog.add_tracking_note(&ticket_id, note)?;
    backlog.write()?;

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

    let pattern = backlog_dir.join("*.md").to_string_lossy().into_owned();

    let paths: Vec<std::path::PathBuf> = glob::glob(&pattern)
        .map_err(|e| format!("glob pattern error: {e}"))?
        .filter_map(|r| r.ok())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n != "template.md" && n != "NEXT.md")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReconcileSummary, SessionDiagnostic};
    use std::path::PathBuf;

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
}
