use crate::{BACKLOG_DIR, LEGACY_SESSION_FILE, SESSION_DIR};
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
}
