use crate::{Backlog, BACKLOG_DIR, LEGACY_SESSION_FILE, SESSION_DIR};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Session {
    pub ticket_id: String,
    pub start: DateTime<Utc>,
    pub file: PathBuf,
    #[serde(default = "default_mode")]
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DerivedStatus {
    Active,
    BatchActive,
    StaleDone,
    StaleBlocked,
    StatusMismatch,
    MissingBacklog,
    MissingTicket,
    InvalidSession,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SessionDiagnostic {
    pub ticket_id: String,
    pub mode: String,
    pub file: PathBuf,
    pub start: Option<DateTime<Utc>>,
    pub derived_status: DerivedStatus,
    pub backlog_status: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct ReconcileSummary {
    pub total_sessions: usize,
    pub active: usize,
    pub batch_active: usize,
    pub stale_done: usize,
    pub stale_blocked: usize,
    pub status_mismatch: usize,
    pub missing_backlog: usize,
    pub missing_ticket: usize,
    pub invalid_session: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReconcileReport {
    pub ok: bool,
    pub sessions: Vec<SessionDiagnostic>,
    pub summary: ReconcileSummary,
}

fn default_mode() -> String {
    "ticket".to_string()
}

impl Session {
    pub fn new(ticket_id: &str, backlog_file: PathBuf) -> Self {
        Self {
            ticket_id: ticket_id.to_uppercase(),
            start: Utc::now(),
            file: backlog_file,
            mode: "ticket".to_string(),
        }
    }

    pub fn new_batch(batch_id: &str, backlog_file: PathBuf) -> Self {
        Self {
            ticket_id: batch_id.to_uppercase(),
            start: Utc::now(),
            file: backlog_file,
            mode: "batch".to_string(),
        }
    }

    /// Compute the session file path for a given key (ticket/batch ID).
    fn session_path(repo_root: &Path, key: &str) -> PathBuf {
        repo_root
            .join(SESSION_DIR)
            .join(format!("{}.yaml", key.to_uppercase()))
    }

    /// Ensure the sessions directory exists.
    fn ensure_session_dir(repo_root: &Path) -> Result<(), String> {
        let dir = repo_root.join(SESSION_DIR);
        if !dir.exists() {
            fs::create_dir_all(&dir)
                .map_err(|e| format!("Failed to create sessions directory: {}", e))?;
        }
        Ok(())
    }

    /// Migrate a legacy `.session` file to the new `.sessions/` directory.
    /// Called automatically by `read` and `list_all` when a legacy file is found.
    pub fn migrate_legacy(repo_root: &Path) -> Result<Option<Self>, String> {
        let legacy_path = repo_root.join(LEGACY_SESSION_FILE);
        if !legacy_path.exists() {
            return Ok(None);
        }

        let contents = fs::read_to_string(&legacy_path)
            .map_err(|e| format!("Failed to read legacy session file: {}", e))?;
        let session: Self = serde_yaml::from_str(&contents)
            .map_err(|e| format!("Failed to parse legacy session file: {}", e))?;

        // Write to new location
        Self::ensure_session_dir(repo_root)?;
        let new_path = Self::session_path(repo_root, &session.ticket_id);
        fs::write(&new_path, &contents)
            .map_err(|e| format!("Failed to write migrated session file: {}", e))?;

        // Remove legacy file
        fs::remove_file(&legacy_path)
            .map_err(|e| format!("Failed to remove legacy session file: {}", e))?;

        eprintln!(
            "Migrated legacy .session to .sessions/{}.yaml",
            session.ticket_id
        );

        Ok(Some(session))
    }

    /// Read a session by its key (ticket/batch ID).
    /// Automatically migrates legacy `.session` file if found.
    pub fn read(repo_root: &Path, key: &str) -> Result<Option<Self>, String> {
        // Try migration first if legacy file exists
        Self::migrate_legacy(repo_root)?;

        let session_path = Self::session_path(repo_root, key);
        if !session_path.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(&session_path)
            .map_err(|e| format!("Failed to read session file: {}", e))?;
        let session: Self = serde_yaml::from_str(&contents)
            .map_err(|e| format!("Failed to parse session file: {}", e))?;
        Ok(Some(session))
    }

    /// List all active sessions.
    /// Automatically migrates legacy `.session` file if found.
    pub fn list_all(repo_root: &Path) -> Result<Vec<Self>, String> {
        // Try migration first if legacy file exists
        Self::migrate_legacy(repo_root)?;

        let dir = repo_root.join(SESSION_DIR);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        let entries =
            fs::read_dir(&dir).map_err(|e| format!("Failed to read sessions directory: {}", e))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "yaml") {
                let contents = fs::read_to_string(&path).map_err(|e| {
                    format!("Failed to read session file {}: {}", path.display(), e)
                })?;
                match serde_yaml::from_str::<Self>(&contents) {
                    Ok(session) => sessions.push(session),
                    Err(e) => {
                        eprintln!(
                            "Warning: skipping invalid session file {}: {}",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }

        // Sort by start time (oldest first)
        sessions.sort_by_key(|s| s.start);
        Ok(sessions)
    }

    fn resolve_backlog_path(repo_root: &Path, backlog_file: &Path) -> PathBuf {
        if backlog_file.is_absolute() {
            backlog_file.to_path_buf()
        } else {
            repo_root.join(backlog_file)
        }
    }

    pub fn reconcile_all(repo_root: &Path) -> Result<ReconcileReport, String> {
        Self::migrate_legacy(repo_root)?;

        let dir = repo_root.join(SESSION_DIR);
        if !dir.exists() {
            return Ok(ReconcileReport {
                ok: true,
                sessions: Vec::new(),
                summary: ReconcileSummary::default(),
            });
        }

        let entries =
            fs::read_dir(&dir).map_err(|e| format!("Failed to read sessions directory: {}", e))?;
        let mut diagnostics = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
            let session_path = entry.path();
            if session_path.extension().is_none_or(|ext| ext != "yaml") {
                continue;
            }

            let contents = fs::read_to_string(&session_path).map_err(|e| {
                format!(
                    "Failed to read session file {}: {}",
                    session_path.display(),
                    e
                )
            })?;

            match serde_yaml::from_str::<Self>(&contents) {
                Ok(session) => diagnostics.push(Self::diagnose_session(repo_root, session)),
                Err(error) => {
                    let ticket_id = session_path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("UNKNOWN")
                        .to_uppercase();
                    diagnostics.push(SessionDiagnostic {
                        ticket_id,
                        mode: "unknown".to_string(),
                        file: session_path,
                        start: None,
                        derived_status: DerivedStatus::InvalidSession,
                        backlog_status: None,
                        message: Some(format!("Failed to parse session YAML: {}", error)),
                    });
                }
            }
        }

        diagnostics.sort_by(|a, b| a.ticket_id.cmp(&b.ticket_id));

        let mut summary = ReconcileSummary::default();
        for diagnostic in &diagnostics {
            summary.total_sessions += 1;
            match diagnostic.derived_status {
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

        let ok = diagnostics.iter().all(|diagnostic| {
            matches!(
                diagnostic.derived_status,
                DerivedStatus::Active | DerivedStatus::BatchActive
            )
        });

        Ok(ReconcileReport {
            ok,
            sessions: diagnostics,
            summary,
        })
    }

    fn diagnose_session(repo_root: &Path, session: Self) -> SessionDiagnostic {
        let backlog_path = Self::resolve_backlog_path(repo_root, &session.file);
        if !backlog_path.exists() {
            return SessionDiagnostic {
                ticket_id: session.ticket_id,
                mode: session.mode,
                file: backlog_path,
                start: Some(session.start),
                derived_status: DerivedStatus::MissingBacklog,
                backlog_status: None,
                message: Some("Referenced backlog file does not exist".to_string()),
            };
        }

        if session.mode == "batch" {
            return SessionDiagnostic {
                ticket_id: session.ticket_id,
                mode: session.mode,
                file: backlog_path,
                start: Some(session.start),
                derived_status: DerivedStatus::BatchActive,
                backlog_status: None,
                message: None,
            };
        }

        match Backlog::read(&backlog_path)
            .and_then(|backlog| backlog.get_status(&session.ticket_id))
        {
            Ok(status) => {
                let (derived_status, message) = match status.as_str() {
                    "In Progress" => (DerivedStatus::Active, None),
                    "Done" => (
                        DerivedStatus::StaleDone,
                        Some("Session is still open but ticket is Done in backlog".to_string()),
                    ),
                    "Blocked" => (
                        DerivedStatus::StaleBlocked,
                        Some("Session is still open but ticket is Blocked in backlog".to_string()),
                    ),
                    other => (
                        DerivedStatus::StatusMismatch,
                        Some(format!(
                            "Session is open but backlog status is '{}' instead of 'In Progress'",
                            other
                        )),
                    ),
                };

                SessionDiagnostic {
                    ticket_id: session.ticket_id,
                    mode: session.mode,
                    file: backlog_path,
                    start: Some(session.start),
                    derived_status,
                    backlog_status: Some(status),
                    message,
                }
            }
            Err(error) => SessionDiagnostic {
                ticket_id: session.ticket_id,
                mode: session.mode,
                file: backlog_path,
                start: Some(session.start),
                derived_status: DerivedStatus::MissingTicket,
                backlog_status: None,
                message: Some(error),
            },
        }
    }

    pub fn write(&self, repo_root: &Path) -> Result<(), String> {
        Self::ensure_session_dir(repo_root)?;
        let session_path = Self::session_path(repo_root, &self.ticket_id);
        let contents = serde_yaml::to_string(self)
            .map_err(|e| format!("Failed to serialize session: {}", e))?;
        fs::write(&session_path, contents)
            .map_err(|e| format!("Failed to write session file: {}", e))?;
        Ok(())
    }

    pub fn remove(repo_root: &Path, key: &str) -> Result<(), String> {
        let session_path = Self::session_path(repo_root, key);
        if session_path.exists() {
            fs::remove_file(&session_path)
                .map_err(|e| format!("Failed to remove session file: {}", e))?;
        }
        Ok(())
    }

    pub fn elapsed(&self) -> chrono::Duration {
        Utc::now() - self.start
    }

    pub fn find_backlog_file(repo_root: &Path, ticket_id: &str) -> Result<PathBuf, String> {
        let pattern = format!("{}/**/*.md", repo_root.join(BACKLOG_DIR).to_string_lossy());
        let entries =
            glob::glob(&pattern).map_err(|e| format!("Failed to glob backlog files: {}", e))?;

        let ticket_id_upper = ticket_id.to_uppercase();
        let ticket_pattern = format!("Ticket {}", ticket_id_upper);

        for path in entries.flatten() {
            if let Ok(contents) = fs::read_to_string(&path) {
                if contents.contains(&ticket_pattern) {
                    return Ok(path);
                }
            }
        }

        Err(format!(
            "Ticket {} not found in any backlog file",
            ticket_id
        ))
    }

    /// Find a backlog file containing any ticket with the given batch prefix.
    /// Searches for `## Ticket <BATCH_ID>-` patterns in backlog files.
    pub fn find_backlog_file_by_batch(repo_root: &Path, batch_id: &str) -> Result<PathBuf, String> {
        let pattern = format!("{}/**/*.md", repo_root.join(BACKLOG_DIR).to_string_lossy());
        let entries =
            glob::glob(&pattern).map_err(|e| format!("Failed to glob backlog files: {}", e))?;

        let batch_upper = batch_id.to_uppercase();
        let ticket_prefix = format!("Ticket {}-", batch_upper);

        for path in entries.flatten() {
            if let Ok(contents) = fs::read_to_string(&path) {
                if contents.contains(&ticket_prefix) {
                    return Ok(path);
                }
            }
        }

        Err(format!(
            "No backlog file found with tickets matching batch prefix '{}'",
            batch_id
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_backlog(repo: &TempDir, name: &str, contents: &str) -> PathBuf {
        let backlog_dir = repo.path().join(BACKLOG_DIR);
        fs::create_dir_all(&backlog_dir).expect("create backlog dir");
        let path = backlog_dir.join(name);
        fs::write(&path, contents).expect("write backlog");
        path
    }

    #[test]
    fn test_find_backlog_file_respects_repo_root() {
        let repo = TempDir::new().expect("create repo tempdir");
        let other = TempDir::new().expect("create other tempdir");
        let backlog_dir = repo.path().join(BACKLOG_DIR).join("tooling");
        fs::create_dir_all(&backlog_dir).expect("create backlog dir");

        let backlog_file = backlog_dir.join("sample.md");
        fs::write(&backlog_file, "## Ticket TEST-1 — Sample\n- Status: Todo\n")
            .expect("write backlog file");

        let original_cwd = std::env::current_dir().expect("get cwd");
        std::env::set_current_dir(other.path()).expect("set cwd");

        let result = Session::find_backlog_file(repo.path(), "TEST-1");

        std::env::set_current_dir(original_cwd).expect("restore cwd");

        assert_eq!(result.expect("find backlog file"), backlog_file);
    }

    #[test]
    fn test_write_read_session_by_key() {
        let repo = TempDir::new().expect("create repo tempdir");
        let session = Session::new("TEST-1", PathBuf::from("backlog.md"));
        session.write(repo.path()).expect("write session");

        let read_back = Session::read(repo.path(), "TEST-1")
            .expect("read session")
            .expect("session should exist");
        assert_eq!(read_back.ticket_id, "TEST-1");

        // Different key should return None
        let other = Session::read(repo.path(), "TEST-2").expect("read other");
        assert!(other.is_none());
    }

    #[test]
    fn test_multiple_concurrent_sessions() {
        let repo = TempDir::new().expect("create repo tempdir");

        let s1 = Session::new("ALPHA-1", PathBuf::from("alpha.md"));
        let s2 = Session::new_batch("BETA", PathBuf::from("beta.md"));

        s1.write(repo.path()).expect("write s1");
        s2.write(repo.path()).expect("write s2");

        let all = Session::list_all(repo.path()).expect("list all");
        assert_eq!(all.len(), 2);

        // Both are independently readable
        assert!(Session::read(repo.path(), "ALPHA-1")
            .expect("read")
            .is_some());
        assert!(Session::read(repo.path(), "BETA").expect("read").is_some());

        // Remove one, other persists
        Session::remove(repo.path(), "ALPHA-1").expect("remove");
        let remaining = Session::list_all(repo.path()).expect("list all");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].ticket_id, "BETA");
    }

    #[test]
    fn test_migrate_legacy_session() {
        let repo = TempDir::new().expect("create repo tempdir");

        let legacy_content =
            "ticket_id: LEGACY-1\nstart: 2026-01-01T00:00:00Z\nfile: legacy.md\nmode: batch\n";
        fs::write(repo.path().join(".session"), legacy_content).expect("write legacy");

        let session = Session::read(repo.path(), "LEGACY-1")
            .expect("read")
            .expect("should find migrated session");
        assert_eq!(session.ticket_id, "LEGACY-1");

        assert!(!repo.path().join(".session").exists());
        assert!(repo.path().join(".sessions/LEGACY-1.yaml").exists());
    }

    #[test]
    fn test_list_all_migrates_legacy() {
        let repo = TempDir::new().expect("create repo tempdir");

        let legacy_content =
            "ticket_id: MIG-1\nstart: 2026-01-01T00:00:00Z\nfile: mig.md\nmode: ticket\n";
        fs::write(repo.path().join(".session"), legacy_content).expect("write legacy");

        let sessions = Session::list_all(repo.path()).expect("list all");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].ticket_id, "MIG-1");
        assert!(!repo.path().join(".session").exists());
    }

    #[test]
    fn test_remove_nonexistent_is_ok() {
        let repo = TempDir::new().expect("create repo tempdir");
        Session::remove(repo.path(), "GHOST-1").expect("remove nonexistent should be ok");
    }

    #[test]
    fn test_reconcile_all_reports_active_ticket_session() {
        let repo = TempDir::new().expect("create repo tempdir");
        let backlog = write_backlog(
            &repo,
            "sample.md",
            "## Ticket TEST-1 — Sample\n- Status: In Progress\n",
        );

        Session::new("TEST-1", backlog)
            .write(repo.path())
            .expect("write session");

        let report = Session::reconcile_all(repo.path()).expect("reconcile");
        assert!(report.ok);
        assert_eq!(report.summary.active, 1);
        assert_eq!(report.sessions[0].derived_status, DerivedStatus::Active);
    }

    #[test]
    fn test_reconcile_all_reports_done_ticket_as_stale() {
        let repo = TempDir::new().expect("create repo tempdir");
        let backlog = write_backlog(
            &repo,
            "sample.md",
            "## Ticket TEST-1 — Sample\n- Status: Done\n",
        );

        Session::new("TEST-1", backlog)
            .write(repo.path())
            .expect("write session");

        let report = Session::reconcile_all(repo.path()).expect("reconcile");
        assert!(!report.ok);
        assert_eq!(report.summary.stale_done, 1);
        assert_eq!(report.sessions[0].derived_status, DerivedStatus::StaleDone);
    }

    #[test]
    fn test_reconcile_all_reports_missing_backlog() {
        let repo = TempDir::new().expect("create repo tempdir");
        Session::new("TEST-1", PathBuf::from("docs/project/backlog/missing.md"))
            .write(repo.path())
            .expect("write session");

        let report = Session::reconcile_all(repo.path()).expect("reconcile");
        assert!(!report.ok);
        assert_eq!(report.summary.missing_backlog, 1);
        assert_eq!(
            report.sessions[0].derived_status,
            DerivedStatus::MissingBacklog
        );
    }

    #[test]
    fn test_reconcile_all_reports_missing_ticket_heading() {
        let repo = TempDir::new().expect("create repo tempdir");
        let backlog = write_backlog(
            &repo,
            "sample.md",
            "## Ticket OTHER-1 — Sample\n- Status: In Progress\n",
        );

        Session::new("TEST-1", backlog)
            .write(repo.path())
            .expect("write session");

        let report = Session::reconcile_all(repo.path()).expect("reconcile");
        assert!(!report.ok);
        assert_eq!(report.summary.missing_ticket, 1);
        assert_eq!(
            report.sessions[0].derived_status,
            DerivedStatus::MissingTicket
        );
    }

    #[test]
    fn test_reconcile_all_reports_invalid_session_yaml() {
        let repo = TempDir::new().expect("create repo tempdir");
        let session_dir = repo.path().join(SESSION_DIR);
        fs::create_dir_all(&session_dir).expect("create sessions dir");
        fs::write(session_dir.join("BROKEN.yaml"), "ticket_id: [").expect("write broken session");

        let report = Session::reconcile_all(repo.path()).expect("reconcile");
        assert!(!report.ok);
        assert_eq!(report.summary.invalid_session, 1);
        assert_eq!(
            report.sessions[0].derived_status,
            DerivedStatus::InvalidSession
        );
    }

    #[test]
    fn test_reconcile_all_reports_batch_session_as_active() {
        let repo = TempDir::new().expect("create repo tempdir");
        let backlog = write_backlog(
            &repo,
            "batch.md",
            "## Ticket BATCH-1 — Sample\n- Status: Todo\n",
        );

        Session::new_batch("BATCH", backlog)
            .write(repo.path())
            .expect("write session");

        let report = Session::reconcile_all(repo.path()).expect("reconcile");
        assert!(report.ok);
        assert_eq!(report.summary.batch_active, 1);
        assert_eq!(
            report.sessions[0].derived_status,
            DerivedStatus::BatchActive
        );
    }
}
