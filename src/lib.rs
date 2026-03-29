use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

pub mod backlog;
pub mod commands;
pub mod session;
pub mod store;

pub use backlog::Backlog;
pub use session::{DerivedStatus, ReconcileReport, ReconcileSummary, Session, SessionDiagnostic};

pub const SESSION_DIR: &str = ".sessions";
pub const BACKLOG_DIR: &str = "docs/project/backlog";

/// Legacy single-file session path (pre-parallel-sessions).
/// Used for automatic migration to the new `.sessions/` directory.
pub const LEGACY_SESSION_FILE: &str = ".session";

/// Resolve the main repository root, starting from `working_dir`.
///
/// The function distinguishes two situations by inspecting the output of
/// `git -C working_dir rev-parse --git-common-dir`:
///
/// - **Relative result** (e.g. `.git`, `../../.git`): we are inside the *main*
///   working tree.  `--show-toplevel` is used to obtain the canonical absolute
///   path, which works correctly from any subdirectory.
/// - **Absolute result** (e.g. `/main-repo/.git`): we are inside a *linked
///   worktree*.  The parent of that absolute path is the main repo root
///   (canonicalized to resolve any symlinks or `..` components).
///
/// If git is unavailable or `working_dir` is not inside a repository, returns
/// `working_dir` unchanged.
pub fn resolve_repo_root_from(working_dir: &Path) -> PathBuf {
    let dir_str = working_dir.to_string_lossy();

    if let Ok(output) = std::process::Command::new("git")
        .args(["-C", &dir_str, "rev-parse", "--git-common-dir"])
        .output()
    {
        if output.status.success() {
            let git_common = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let git_common_path = PathBuf::from(&git_common);

            if !git_common_path.is_absolute() {
                // Relative path → main working tree (from any subdirectory).
                // --show-toplevel always returns an absolute path.
                if let Ok(toplevel) = std::process::Command::new("git")
                    .args(["-C", &dir_str, "rev-parse", "--show-toplevel"])
                    .output()
                {
                    if toplevel.status.success() {
                        return PathBuf::from(String::from_utf8_lossy(&toplevel.stdout).trim());
                    }
                }
            } else {
                // Absolute path → linked worktree.  The common .git dir's parent
                // is the main repo root; canonicalize to resolve any symlinks.
                if let Some(parent) = git_common_path.parent() {
                    return parent
                        .canonicalize()
                        .unwrap_or_else(|_| parent.to_path_buf());
                }
            }
        }
    }

    working_dir.to_path_buf()
}

/// Resolve the main repository root from the current working directory.
///
/// Calls [`resolve_repo_root_from`] with the process CWD.  Falls back to `"."`
/// if the CWD cannot be determined.
pub fn resolve_repo_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_repo_root_from(&cwd)
}

#[derive(Parser)]
#[command(name = "ticket", about = "Ticket tracking CLI for backlog governance")]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Repository root override. Defaults to the main git repo root (auto-detected).
    #[arg(long, global = true)]
    repo_root: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    Start {
        #[arg(help = "Ticket or batch ID (e.g., FLUX-123 or ORCH-MCP)")]
        ticket_id: String,
        #[arg(
            long,
            help = "Start a batch-level session covering all tickets in the backlog file"
        )]
        batch: bool,
    },
    Done {
        #[arg(help = "Ticket or batch ID (e.g., FLUX-123 or ORCH-MCP)")]
        ticket_id: String,
        #[arg(long, help = "Close a batch-level session")]
        batch: bool,
    },
    Status,
    Reconcile {
        #[arg(long, help = "Emit reconciliation results as JSON")]
        json: bool,
    },
    Blocked {
        #[arg(help = "Ticket ID (e.g., FLUX-123)")]
        ticket_id: String,
        #[arg(help = "Reason for blocking")]
        reason: String,
    },
    Note {
        #[arg(help = "Ticket ID (e.g., FLUX-123)")]
        ticket_id: String,
        #[arg(help = "Note to add")]
        note: String,
    },
}

pub fn run(cli: Cli) -> Result<(), String> {
    // Use the explicit --repo-root when provided; otherwise auto-detect.
    let repo_root = cli.repo_root.unwrap_or_else(resolve_repo_root);

    match cli.command {
        Commands::Start { ticket_id, batch } => {
            if batch {
                commands::start_batch(&repo_root, &ticket_id)
            } else {
                commands::start(&repo_root, &ticket_id)
            }
        }
        Commands::Done { ticket_id, batch } => {
            if batch {
                commands::done_batch(&repo_root, &ticket_id)
            } else {
                commands::done(&repo_root, &ticket_id)
            }
        }
        Commands::Status => commands::status(&repo_root),
        Commands::Reconcile { json } => commands::reconcile(&repo_root, json),
        Commands::Blocked { ticket_id, reason } => {
            commands::blocked(&repo_root, &ticket_id, &reason)
        }
        Commands::Note { ticket_id, note } => commands::note(&repo_root, &ticket_id, &note),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn test_resolve_repo_root_from_returns_existing_path() {
        // Use the CARGO_MANIFEST_DIR (the directory containing Cargo.toml)
        // which is stable and always exists — avoids CWD race with other tests.
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = resolve_repo_root_from(&manifest_dir);
        assert!(
            root.exists(),
            "resolve_repo_root_from() returned a non-existent path: {}",
            root.display()
        );
    }

    #[test]
    fn test_resolve_repo_root_from_fallback_outside_git() {
        // A fresh system temp dir is not a git repo, so the function should
        // return working_dir unchanged — no CWD mutation needed.
        let tmp = tempfile::TempDir::new().expect("create tempdir");
        let root = resolve_repo_root_from(tmp.path());
        assert_eq!(
            root,
            tmp.path(),
            "should return working_dir unchanged when not inside a git repo"
        );
    }

    #[test]
    fn test_resolve_repo_root_from_worktree() {
        // Create a temporary main repo with one commit.
        let main_repo = tempfile::TempDir::new().expect("create main repo tempdir");

        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(main_repo.path())
                // Suppress output so test logs stay clean.
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap_or_else(|e| panic!("git {:?}: {}", args, e));
            assert!(status.success(), "git {:?} exited with {}", args, status);
        };

        git(&["init"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(main_repo.path().join("README"), "init").expect("write README");
        git(&["add", "."]);
        git(&["commit", "-m", "init"]);

        // Create a linked worktree at a path that does not yet exist.
        let worktree_parent = tempfile::TempDir::new().expect("create worktree parent");
        let wt_path = worktree_parent.path().join("wt");

        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().expect("wt path to str"),
                "--detach",
            ])
            .current_dir(main_repo.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git worktree add");
        assert!(status.success(), "git worktree add failed");

        // Resolving from the linked worktree must return the main repo root.
        let resolved = resolve_repo_root_from(&wt_path);
        let canonical_main = main_repo.path().canonicalize().expect("canonicalize main");
        assert_eq!(
            resolved, canonical_main,
            "from a linked worktree, should resolve to the main repo root"
        );
    }
}
