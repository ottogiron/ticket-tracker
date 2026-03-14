use crate::{Backlog, Session};
use chrono::Utc;
use std::path::Path;

fn format_duration(duration: chrono::Duration) -> String {
    let total_secs = duration.num_seconds();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
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
    let sessions = Session::list_all(repo_root)?;

    if sessions.is_empty() {
        println!("No active sessions");
        println!("Run 'ticket start <ticket-id>' to begin work.");
        return Ok(());
    }

    for (i, session) in sessions.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let elapsed = session.elapsed();
        if session.mode == "batch" {
            println!("Batch: {}", session.ticket_id);
            println!("Mode: batch");
        } else {
            println!("Ticket: {}", session.ticket_id);
            println!("Mode: ticket");
        }
        println!("Status: In Progress");
        println!("Started: {}", session.start.format("%Y-%m-%d %H:%M UTC"));
        println!("Elapsed: {}", format_duration(elapsed));
        println!("File: {}", session.file.display());
    }

    if sessions.len() > 1 {
        println!();
        println!("{} active sessions", sessions.len());
    }

    Ok(())
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
