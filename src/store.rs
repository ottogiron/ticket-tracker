//! SQLite-backed store for ticket, session, and note persistence.
//!
//! Opens (or creates) `.ticket/state.db` relative to the repository root.
//! WAL mode and foreign-key enforcement are enabled on every connection.
//! All schema migrations are idempotent (`CREATE TABLE IF NOT EXISTS`).

use rusqlite::{params, Connection};
use std::path::Path;

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Ticket {
    pub ticket_id: String,
    pub backlog: String,
    pub title: String,
    pub spec_file: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct TicketStatus {
    pub ticket_id: String,
    pub status: String,
    pub blocked_reason: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub duration_secs: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: i64,
    pub ticket_id: String,
    pub mode: String,
    pub batch_id: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TicketNote {
    pub id: i64,
    pub ticket_id: String,
    pub kind: String,
    pub body: String,
    pub created_at: i64,
}

/// A ticket row paired with its optional status row.
#[derive(Debug, Clone)]
pub struct TicketWithStatus {
    pub ticket: Ticket,
    pub status: Option<TicketStatus>,
}

// ── Store ─────────────────────────────────────────────────────────────────────

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (or create) the store at `{repo_root}/.ticket/state.db`.
    ///
    /// Enables WAL mode, foreign-key enforcement, and runs all schema migrations
    /// before returning.
    pub fn open(repo_root: &Path) -> Result<Store, String> {
        let db_dir = repo_root.join(".ticket");
        std::fs::create_dir_all(&db_dir)
            .map_err(|e| format!("failed to create .ticket dir: {e}"))?;
        let db_path = db_dir.join("state.db");
        let conn =
            Connection::open(&db_path).map_err(|e| format!("failed to open database: {e}"))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys = ON;")
            .map_err(|e| format!("failed to configure connection: {e}"))?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS tickets (
                    ticket_id   TEXT PRIMARY KEY,
                    backlog     TEXT NOT NULL,
                    title       TEXT NOT NULL,
                    spec_file   TEXT,
                    created_at  INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS ticket_status (
                    ticket_id       TEXT PRIMARY KEY REFERENCES tickets(ticket_id) ON DELETE CASCADE,
                    status          TEXT NOT NULL DEFAULT 'todo',
                    blocked_reason  TEXT,
                    started_at      INTEGER,
                    finished_at     INTEGER,
                    duration_secs   INTEGER,
                    updated_at      INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS sessions (
                    session_id  INTEGER PRIMARY KEY AUTOINCREMENT,
                    ticket_id   TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
                    mode        TEXT NOT NULL DEFAULT 'normal',
                    batch_id    TEXT,
                    started_at  INTEGER NOT NULL,
                    ended_at    INTEGER
                );

                CREATE TABLE IF NOT EXISTS ticket_notes (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    ticket_id   TEXT NOT NULL REFERENCES tickets(ticket_id) ON DELETE CASCADE,
                    kind        TEXT NOT NULL,
                    body        TEXT NOT NULL,
                    created_at  INTEGER NOT NULL
                );
                ",
            )
            .map_err(|e| format!("schema migration failed: {e}"))
    }

    // ── Ticket ────────────────────────────────────────────────────────────────

    pub fn insert_ticket(&self, ticket: &Ticket) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO tickets (ticket_id, backlog, title, spec_file, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    ticket.ticket_id,
                    ticket.backlog,
                    ticket.title,
                    ticket.spec_file,
                    ticket.created_at,
                ],
            )
            .map_err(|e| format!("insert_ticket failed: {e}"))?;
        Ok(())
    }

    pub fn get_ticket(&self, ticket_id: &str) -> Result<Option<Ticket>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT ticket_id, backlog, title, spec_file, created_at
                 FROM tickets WHERE ticket_id = ?1",
            )
            .map_err(|e| format!("prepare get_ticket: {e}"))?;
        let mut rows = stmt
            .query_map(params![ticket_id], |row| {
                Ok(Ticket {
                    ticket_id: row.get(0)?,
                    backlog: row.get(1)?,
                    title: row.get(2)?,
                    spec_file: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|e| format!("get_ticket query: {e}"))?;
        rows.next()
            .transpose()
            .map_err(|e| format!("get_ticket row: {e}"))
    }

    pub fn list_tickets(&self) -> Result<Vec<Ticket>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT ticket_id, backlog, title, spec_file, created_at
                 FROM tickets ORDER BY created_at",
            )
            .map_err(|e| format!("prepare list_tickets: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Ticket {
                    ticket_id: row.get(0)?,
                    backlog: row.get(1)?,
                    title: row.get(2)?,
                    spec_file: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|e| format!("list_tickets query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("list_tickets collect: {e}"))
    }

    pub fn delete_ticket(&self, ticket_id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM tickets WHERE ticket_id = ?1",
                params![ticket_id],
            )
            .map_err(|e| format!("delete_ticket failed: {e}"))?;
        Ok(())
    }

    // ── TicketStatus ──────────────────────────────────────────────────────────

    /// Insert or update the status row for a ticket.
    pub fn upsert_ticket_status(&self, status: &TicketStatus) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO ticket_status
                    (ticket_id, status, blocked_reason, started_at, finished_at,
                     duration_secs, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(ticket_id) DO UPDATE SET
                    status          = excluded.status,
                    blocked_reason  = excluded.blocked_reason,
                    started_at      = excluded.started_at,
                    finished_at     = excluded.finished_at,
                    duration_secs   = excluded.duration_secs,
                    updated_at      = excluded.updated_at",
                params![
                    status.ticket_id,
                    status.status,
                    status.blocked_reason,
                    status.started_at,
                    status.finished_at,
                    status.duration_secs,
                    status.updated_at,
                ],
            )
            .map_err(|e| format!("upsert_ticket_status failed: {e}"))?;
        Ok(())
    }

    pub fn get_ticket_status(&self, ticket_id: &str) -> Result<Option<TicketStatus>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT ticket_id, status, blocked_reason, started_at, finished_at,
                        duration_secs, updated_at
                 FROM ticket_status WHERE ticket_id = ?1",
            )
            .map_err(|e| format!("prepare get_ticket_status: {e}"))?;
        let mut rows = stmt
            .query_map(params![ticket_id], |row| {
                Ok(TicketStatus {
                    ticket_id: row.get(0)?,
                    status: row.get(1)?,
                    blocked_reason: row.get(2)?,
                    started_at: row.get(3)?,
                    finished_at: row.get(4)?,
                    duration_secs: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })
            .map_err(|e| format!("get_ticket_status query: {e}"))?;
        rows.next()
            .transpose()
            .map_err(|e| format!("get_ticket_status row: {e}"))
    }

    /// Fetch a ticket and its optional status row as a combined value.
    pub fn ticket_with_status(&self, ticket_id: &str) -> Result<Option<TicketWithStatus>, String> {
        match self.get_ticket(ticket_id)? {
            None => Ok(None),
            Some(ticket) => {
                let status = self.get_ticket_status(ticket_id)?;
                Ok(Some(TicketWithStatus { ticket, status }))
            }
        }
    }

    // ── Session ───────────────────────────────────────────────────────────────

    /// Insert a new session row and return the generated `session_id`.
    pub fn insert_session(
        &self,
        ticket_id: &str,
        mode: &str,
        batch_id: Option<&str>,
        started_at: i64,
    ) -> Result<i64, String> {
        self.conn
            .execute(
                "INSERT INTO sessions (ticket_id, mode, batch_id, started_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![ticket_id, mode, batch_id, started_at],
            )
            .map_err(|e| format!("insert_session failed: {e}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_session(&self, session_id: i64) -> Result<Option<Session>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT session_id, ticket_id, mode, batch_id, started_at, ended_at
                 FROM sessions WHERE session_id = ?1",
            )
            .map_err(|e| format!("prepare get_session: {e}"))?;
        let mut rows = stmt
            .query_map(params![session_id], |row| {
                Ok(Session {
                    session_id: row.get(0)?,
                    ticket_id: row.get(1)?,
                    mode: row.get(2)?,
                    batch_id: row.get(3)?,
                    started_at: row.get(4)?,
                    ended_at: row.get(5)?,
                })
            })
            .map_err(|e| format!("get_session query: {e}"))?;
        rows.next()
            .transpose()
            .map_err(|e| format!("get_session row: {e}"))
    }

    /// List all sessions that have not yet been closed (`ended_at IS NULL`).
    pub fn active_sessions(&self) -> Result<Vec<Session>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT session_id, ticket_id, mode, batch_id, started_at, ended_at
                 FROM sessions WHERE ended_at IS NULL ORDER BY started_at",
            )
            .map_err(|e| format!("prepare active_sessions: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Session {
                    session_id: row.get(0)?,
                    ticket_id: row.get(1)?,
                    mode: row.get(2)?,
                    batch_id: row.get(3)?,
                    started_at: row.get(4)?,
                    ended_at: row.get(5)?,
                })
            })
            .map_err(|e| format!("active_sessions query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("active_sessions collect: {e}"))
    }

    /// Mark a session as closed by setting its `ended_at` timestamp.
    pub fn close_session(&self, session_id: i64, end_time: i64) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE sessions SET ended_at = ?1 WHERE session_id = ?2",
                params![end_time, session_id],
            )
            .map_err(|e| format!("close_session failed: {e}"))?;
        Ok(())
    }

    // ── TicketNote ────────────────────────────────────────────────────────────

    /// Append a note to a ticket.  Returns the generated note `id`.
    pub fn add_note(
        &self,
        ticket_id: &str,
        kind: &str,
        body: &str,
        created_at: i64,
    ) -> Result<i64, String> {
        self.conn
            .execute(
                "INSERT INTO ticket_notes (ticket_id, kind, body, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![ticket_id, kind, body, created_at],
            )
            .map_err(|e| format!("add_note failed: {e}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_notes(&self, ticket_id: &str) -> Result<Vec<TicketNote>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, ticket_id, kind, body, created_at
                 FROM ticket_notes WHERE ticket_id = ?1 ORDER BY created_at",
            )
            .map_err(|e| format!("prepare list_notes: {e}"))?;
        let rows = stmt
            .query_map(params![ticket_id], |row| {
                Ok(TicketNote {
                    id: row.get(0)?,
                    ticket_id: row.get(1)?,
                    kind: row.get(2)?,
                    body: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|e| format!("list_notes query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("list_notes collect: {e}"))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store() -> (TempDir, Store) {
        let dir = TempDir::new().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        (dir, store)
    }

    fn sample_ticket() -> Ticket {
        Ticket {
            ticket_id: "FLUX-1".to_string(),
            backlog: "project".to_string(),
            title: "Test ticket".to_string(),
            spec_file: None,
            created_at: 1_000_000,
        }
    }

    // ── open ──────────────────────────────────────────────────────────────────

    #[test]
    fn test_open_creates_db_file() {
        let dir = TempDir::new().expect("tempdir");
        Store::open(dir.path()).expect("open store");
        assert!(dir.path().join(".ticket").join("state.db").exists());
    }

    #[test]
    fn test_open_idempotent() {
        let dir = TempDir::new().expect("tempdir");
        Store::open(dir.path()).expect("first open");
        Store::open(dir.path()).expect("second open — must not fail");
    }

    // ── Ticket ────────────────────────────────────────────────────────────────

    #[test]
    fn test_insert_ticket_and_get() {
        let (_dir, store) = make_store();
        store.insert_ticket(&sample_ticket()).expect("insert");
        let got = store.get_ticket("FLUX-1").expect("get").expect("some");
        assert_eq!(got.ticket_id, "FLUX-1");
        assert_eq!(got.title, "Test ticket");
        assert_eq!(got.backlog, "project");
        assert!(got.spec_file.is_none());
    }

    #[test]
    fn test_insert_ticket_with_spec_file() {
        let (_dir, store) = make_store();
        let t = Ticket {
            spec_file: Some("docs/spec.md".to_string()),
            ..sample_ticket()
        };
        store.insert_ticket(&t).expect("insert");
        let got = store.get_ticket("FLUX-1").expect("get").expect("some");
        assert_eq!(got.spec_file.as_deref(), Some("docs/spec.md"));
    }

    #[test]
    fn test_get_ticket_missing_returns_none() {
        let (_dir, store) = make_store();
        assert!(store.get_ticket("MISSING").expect("get").is_none());
    }

    #[test]
    fn test_list_tickets_empty() {
        let (_dir, store) = make_store();
        assert!(store.list_tickets().expect("list").is_empty());
    }

    #[test]
    fn test_list_tickets_multiple_ordered_by_created_at() {
        let (_dir, store) = make_store();
        store.insert_ticket(&sample_ticket()).expect("insert 1");
        store
            .insert_ticket(&Ticket {
                ticket_id: "FLUX-2".to_string(),
                created_at: 1_000_001,
                ..sample_ticket()
            })
            .expect("insert 2");
        let tickets = store.list_tickets().expect("list");
        assert_eq!(tickets.len(), 2);
        assert_eq!(tickets[0].ticket_id, "FLUX-1");
        assert_eq!(tickets[1].ticket_id, "FLUX-2");
    }

    #[test]
    fn test_delete_ticket() {
        let (_dir, store) = make_store();
        store.insert_ticket(&sample_ticket()).expect("insert");
        store.delete_ticket("FLUX-1").expect("delete");
        assert!(store.get_ticket("FLUX-1").expect("get").is_none());
    }

    #[test]
    fn test_delete_ticket_cascades_child_rows() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        store
            .upsert_ticket_status(&TicketStatus {
                ticket_id: "FLUX-1".to_string(),
                status: "todo".to_string(),
                blocked_reason: None,
                started_at: None,
                finished_at: None,
                duration_secs: None,
                updated_at: 1_000_000,
            })
            .expect("upsert status");
        store
            .insert_session("FLUX-1", "normal", None, 2_000_000)
            .expect("insert session");
        store
            .add_note("FLUX-1", "comment", "a note", 3_000_000)
            .expect("add note");

        store.delete_ticket("FLUX-1").expect("delete ticket");

        assert!(store.get_ticket("FLUX-1").expect("ticket gone").is_none());
        assert!(store
            .get_ticket_status("FLUX-1")
            .expect("status gone")
            .is_none());
        assert!(store.active_sessions().expect("sessions gone").is_empty());
        assert!(store.list_notes("FLUX-1").expect("notes gone").is_empty());
    }

    // ── TicketStatus ──────────────────────────────────────────────────────────

    #[test]
    fn test_upsert_ticket_status_insert() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        let s = TicketStatus {
            ticket_id: "FLUX-1".to_string(),
            status: "in-progress".to_string(),
            blocked_reason: None,
            started_at: Some(1_000_000),
            finished_at: None,
            duration_secs: None,
            updated_at: 1_000_001,
        };
        store.upsert_ticket_status(&s).expect("upsert");
        let got = store
            .get_ticket_status("FLUX-1")
            .expect("get")
            .expect("some");
        assert_eq!(got.status, "in-progress");
        assert_eq!(got.started_at, Some(1_000_000));
        assert!(got.finished_at.is_none());
    }

    #[test]
    fn test_upsert_ticket_status_update() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        let s = TicketStatus {
            ticket_id: "FLUX-1".to_string(),
            status: "in-progress".to_string(),
            blocked_reason: None,
            started_at: Some(1_000_000),
            finished_at: None,
            duration_secs: None,
            updated_at: 1_000_001,
        };
        store.upsert_ticket_status(&s).expect("upsert 1");
        let s2 = TicketStatus {
            status: "done".to_string(),
            finished_at: Some(1_000_100),
            duration_secs: Some(100),
            updated_at: 1_000_101,
            ..s
        };
        store.upsert_ticket_status(&s2).expect("upsert 2");
        let got = store
            .get_ticket_status("FLUX-1")
            .expect("get")
            .expect("some");
        assert_eq!(got.status, "done");
        assert_eq!(got.finished_at, Some(1_000_100));
        assert_eq!(got.duration_secs, Some(100));
    }

    #[test]
    fn test_get_ticket_status_missing_returns_none() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        assert!(store.get_ticket_status("FLUX-1").expect("get").is_none());
    }

    #[test]
    fn test_ticket_with_status_no_status() {
        let (_dir, store) = make_store();
        store.insert_ticket(&sample_ticket()).expect("insert");
        let tws = store
            .ticket_with_status("FLUX-1")
            .expect("get")
            .expect("some");
        assert_eq!(tws.ticket.ticket_id, "FLUX-1");
        assert!(tws.status.is_none());
    }

    #[test]
    fn test_ticket_with_status_with_status() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        store
            .upsert_ticket_status(&TicketStatus {
                ticket_id: "FLUX-1".to_string(),
                status: "todo".to_string(),
                blocked_reason: None,
                started_at: None,
                finished_at: None,
                duration_secs: None,
                updated_at: 1_000_000,
            })
            .expect("upsert");
        let tws = store
            .ticket_with_status("FLUX-1")
            .expect("get")
            .expect("some");
        assert_eq!(tws.status.expect("status").status, "todo");
    }

    #[test]
    fn test_ticket_with_status_missing_ticket() {
        let (_dir, store) = make_store();
        assert!(store.ticket_with_status("MISSING").expect("get").is_none());
    }

    // ── Session ───────────────────────────────────────────────────────────────

    #[test]
    fn test_insert_session_and_get() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        let sid = store
            .insert_session("FLUX-1", "normal", None, 2_000_000)
            .expect("insert session");
        let sess = store.get_session(sid).expect("get").expect("some");
        assert_eq!(sess.ticket_id, "FLUX-1");
        assert_eq!(sess.mode, "normal");
        assert!(sess.batch_id.is_none());
        assert!(sess.ended_at.is_none());
    }

    #[test]
    fn test_insert_session_with_batch_id() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        let sid = store
            .insert_session("FLUX-1", "batch", Some("ORCH-MCP"), 2_000_000)
            .expect("insert session");
        let sess = store.get_session(sid).expect("get").expect("some");
        assert_eq!(sess.batch_id.as_deref(), Some("ORCH-MCP"));
    }

    #[test]
    fn test_get_session_missing_returns_none() {
        let (_dir, store) = make_store();
        assert!(store.get_session(9999).expect("get").is_none());
    }

    #[test]
    fn test_active_sessions_filters_closed() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        let sid1 = store
            .insert_session("FLUX-1", "normal", None, 2_000_000)
            .expect("insert 1");
        let _sid2 = store
            .insert_session("FLUX-1", "normal", None, 2_000_001)
            .expect("insert 2");
        store.close_session(sid1, 2_000_100).expect("close");
        let active = store.active_sessions().expect("active");
        assert_eq!(active.len(), 1);
        assert!(active[0].ended_at.is_none());
    }

    #[test]
    fn test_active_sessions_empty_when_all_closed() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        let sid = store
            .insert_session("FLUX-1", "normal", None, 2_000_000)
            .expect("insert");
        store.close_session(sid, 2_000_999).expect("close");
        assert!(store.active_sessions().expect("active").is_empty());
    }

    #[test]
    fn test_close_session_sets_ended_at() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        let sid = store
            .insert_session("FLUX-1", "normal", None, 2_000_000)
            .expect("insert session");
        store.close_session(sid, 2_000_999).expect("close");
        let sess = store.get_session(sid).expect("get").expect("some");
        assert_eq!(sess.ended_at, Some(2_000_999));
    }

    // ── TicketNote ────────────────────────────────────────────────────────────

    #[test]
    fn test_add_note_and_list() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        store
            .add_note("FLUX-1", "comment", "First note", 3_000_000)
            .expect("add note 1");
        store
            .add_note("FLUX-1", "blocker", "Second note", 3_000_001)
            .expect("add note 2");
        let notes = store.list_notes("FLUX-1").expect("list notes");
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].kind, "comment");
        assert_eq!(notes[0].body, "First note");
        assert_eq!(notes[1].kind, "blocker");
        assert_eq!(notes[1].body, "Second note");
    }

    #[test]
    fn test_list_notes_empty() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        assert!(store.list_notes("FLUX-1").expect("list").is_empty());
    }

    #[test]
    fn test_add_note_returns_incrementing_ids() {
        let (_dir, store) = make_store();
        store
            .insert_ticket(&sample_ticket())
            .expect("insert ticket");
        let id1 = store
            .add_note("FLUX-1", "comment", "a", 3_000_000)
            .expect("note 1");
        let id2 = store
            .add_note("FLUX-1", "comment", "b", 3_000_001)
            .expect("note 2");
        assert!(id2 > id1);
    }
}
