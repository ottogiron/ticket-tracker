use crate::{Backlog, DerivedStatus, ReconcileReport, Session};
use chrono::Utc;
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
}
