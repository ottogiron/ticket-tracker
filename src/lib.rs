use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub mod backlog;
pub mod commands;
pub mod session;

pub use backlog::Backlog;
pub use session::Session;

pub const SESSION_DIR: &str = ".sessions";
pub const BACKLOG_DIR: &str = "docs/project/backlog";

/// Legacy single-file session path (pre-parallel-sessions).
/// Used for automatic migration to the new `.sessions/` directory.
pub const LEGACY_SESSION_FILE: &str = ".session";

#[derive(Parser)]
#[command(name = "ticket", about = "Ticket tracking CLI for backlog governance")]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, global = true, default_value = ".")]
    repo_root: PathBuf,
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
    match cli.command {
        Commands::Start { ticket_id, batch } => {
            if batch {
                commands::start_batch(&cli.repo_root, &ticket_id)
            } else {
                commands::start(&cli.repo_root, &ticket_id)
            }
        }
        Commands::Done { ticket_id, batch } => {
            if batch {
                commands::done_batch(&cli.repo_root, &ticket_id)
            } else {
                commands::done(&cli.repo_root, &ticket_id)
            }
        }
        Commands::Status => commands::status(&cli.repo_root),
        Commands::Blocked { ticket_id, reason } => {
            commands::blocked(&cli.repo_root, &ticket_id, &reason)
        }
        Commands::Note { ticket_id, note } => commands::note(&cli.repo_root, &ticket_id, &note),
    }
}
