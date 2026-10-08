//! Looks at a running e-ink home display server and changes it, from the same machine, over its admin socket.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use chrono::{Local, Utc};
use clap::{Parser, Subcommand};
use eink_home_display_rust::adapters::admin::{AdminClient, CallError, describe, describe_window};
use eink_home_display_rust::bootstrap::load_application_config;

/// Talks to a running server's admin interface. The server offers it when it serves HTTPS
/// (`transport = "prefer-https"` or `"https"`).
#[derive(Parser)]
#[command(name = "displayctl")]
struct Args {
    /// The server's configuration file, which says where its admin socket is. A relative directory in it is
    /// taken from where this command is run, so run it from the server's own working directory, or use --socket.
    #[arg(
        short,
        long,
        required_unless_present = "socket",
        conflicts_with = "socket"
    )]
    config_file: Option<PathBuf>,

    /// The admin socket itself, instead of reading its place from the configuration
    #[arg(long)]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// The pairing window: while it is open, a display that has not joined can ask to
    Window {
        #[command(subcommand)]
        action: WindowAction,
    },
}

#[derive(Subcommand)]
enum WindowAction {
    /// Say whether it is open
    Show,
    /// Open it, so a display can ask to join. Opening an open window sets the time afresh.
    Open {
        /// How long to open it for, in minutes (1 to 240)
        #[arg(default_value_t = 15)]
        minutes: u32,
    },
    /// Close it. A display already waiting can still be approved.
    Close,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), String> {
    let socket = match (&args.socket, &args.config_file) {
        (Some(socket), _) => socket.clone(),
        (None, Some(path)) => load_application_config(path)
            .with_context(|| format!("Could not read {}", path.display()))
            .map_err(|e| format!("{e:#}"))?
            .server
            .admin_socket(),
        (None, None) => unreachable!("clap requires one of them"),
    };
    let client = AdminClient::new(&socket);
    let failed = |error: CallError| match error {
        CallError::Unreachable(e) => format!(
            "Could not reach the server's admin socket at {}.\n\
             Is the server running, with transport = \"prefer-https\" or \"https\" and [server.admin] enabled? \
             Are you the user it runs as, and in its working directory (or using --socket)?\n({e:#})",
            socket.display()
        ),
        other => describe(&other),
    };
    match args.command {
        Command::Window { action } => {
            let state = match action {
                WindowAction::Show => client.window().await,
                WindowAction::Open { minutes } => client.open_window(minutes).await,
                WindowAction::Close => client.close_window().await,
            }
            .map_err(failed)?;
            println!("{}", describe_window(&state, Utc::now(), &Local));
            Ok(())
        }
    }
}
