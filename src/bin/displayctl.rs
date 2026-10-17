//! Looks at a running home display server and changes it, from the same machine, over its admin socket.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use chrono::{Local, Utc};
use clap::{Parser, Subcommand};
use home_display_server::adapters::admin::{
    AdminClient, CallError, IssueViewer, approved, credential_changed, describe,
    describe_credentials, describe_entry, describe_layouts, describe_list, describe_viewers,
    describe_window, forgotten, key_from_stream, layout_changed, qr_for_terminal, rejected,
    revoked, viewer_changed, viewer_issued,
};
use home_display_server::bootstrap::load_application_config;

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
    /// The displays the server knows, and where each stands
    Displays {
        #[command(subcommand)]
        action: DisplaysAction,
    },
    /// The layouts there are, and which one a display is given
    Layout {
        #[command(subcommand)]
        action: LayoutAction,
    },
    /// Who may look at the picture in a web browser (when `[server.web]` needs a token)
    Viewer {
        #[command(subcommand)]
        action: ViewerAction,
    },
    /// The keys the server uses to ask other services for data: set them, or take them away, while it runs
    ///
    /// A key can be set and never read back, so it need not be in the configuration file. By default it lives in the
    /// server's memory alone and is gone when the server stops; `--remember` also keeps it in a file only the server's
    /// user can read.
    Credential {
        #[command(subcommand)]
        action: CredentialAction,
    },
    /// Approve a display that is waiting, with the code shown on its own panel
    ///
    /// Type the code from the display itself, never from anywhere else: the server does not show it, because a code
    /// copied from the server would say nothing about the display. Any case, dashes optional; I and L read as 1,
    /// O as 0.
    Approve {
        /// The display's name, as `displays list` shows it
        name: String,
        /// The code on the display's own panel, like JGWP-14YW-3BT0
        code: String,
    },
    /// Turn a waiting display down, until it is forgotten. For a member with another key waiting to take its
    /// name, turn that key down and leave the member as it was.
    Reject {
        /// The display's name
        name: String,
    },
    /// End a member's membership: it is turned away from its very next request
    Revoke {
        /// The display's name
        name: String,
    },
    /// Forget a display, whatever its state, so it can ask again as if for the first time
    Forget {
        /// The display's name
        name: String,
    },
}

#[derive(Subcommand)]
enum CredentialAction {
    /// The keys there are, and whether each is set. Never a key
    List,
    /// Set a key, which the server uses from its next request to that service
    ///
    /// The key is typed at a prompt that does not show it, or piped in (`pass show openweather | displayctl credential set
    /// open-weather`). It is never taken from the command line, where it would be in the shell's history and visible to
    /// other users.
    Set {
        /// Which key: open-weather, open-ldbws or tfl (`credential list` shows them)
        name: String,
        /// Also keep the key in a file only the server's user can read, so it is there after a restart. Without this
        /// the key is in memory alone, and setting it drops any copy kept before.
        #[arg(long)]
        remember: bool,
    },
    /// Take away the key that was set, and any copy kept of it. A key in the configuration file is used again
    Clear {
        /// Which key
        name: String,
    },
}

#[derive(Subcommand)]
enum DisplaysAction {
    /// Every display, with where it stands
    List,
    /// One display
    Show {
        /// The display's name
        name: String,
    },
}

#[derive(Subcommand)]
enum LayoutAction {
    /// The layouts there are, the default first
    List,
    /// Give a display a layout, which it is sent from its next check-in when it doesn't ask for another itself
    Set {
        /// The display's name
        name: String,
        /// The layout, as `layout list` shows it
        layout: String,
    },
    /// Take a display's layout away, so it is sent the default again
    Clear {
        /// The display's name
        name: String,
    },
}

#[derive(Subcommand)]
enum ViewerAction {
    /// Make a viewer and print the link that signs a browser in, once
    ///
    /// The link works once and only for a short time (15 minutes unless told). Open it in the browser that is to look at the
    /// picture: it trades the code in the link for a token that browser keeps. The link is not shown again.
    Create {
        /// What to call it, to tell it from the others: "kitchen tablet"
        #[arg(short, long)]
        label: String,
        /// A layout it may look at, as `layout list` shows it. Repeat for several. Left out, all of them.
        #[arg(long = "layout")]
        layouts: Vec<String>,
        /// How long its token lasts once the link is used, in days (1 to 3650); with --refresh, how long the session lasts
        #[arg(long, value_name = "DAYS")]
        lifetime_days: Option<u32>,
        /// Give it short-lived access tokens renewed with a refresh token that changes each time, so that a lost token is
        /// good for an hour and a copied refresh token is noticed and ends the session
        #[arg(long)]
        refresh: bool,
        /// How long the link works, in minutes (1 to 1440)
        #[arg(long, value_name = "MINUTES")]
        code_minutes: Option<u32>,
        /// Print the link as a QR code too, to scan with a tablet or phone instead of typing it. Done anyway when the
        /// output is a terminal.
        #[arg(long, conflicts_with = "no_qr")]
        qr: bool,
        /// Do not print the QR code, even to a terminal
        #[arg(long)]
        no_qr: bool,
    },
    /// Every viewer, with where it stands
    List,
    /// End a viewer's access now: its link stops working and its token stops being accepted
    Revoke {
        /// The viewer's id, as `viewer list` shows it
        id: String,
    },
    /// Forget a viewer altogether, which frees its place
    Forget {
        /// The viewer's id
        id: String,
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
        Command::Displays { action } => {
            match action {
                DisplaysAction::List => {
                    let list = client.displays().await.map_err(failed)?;
                    println!("{}", describe_list(&list, Utc::now(), &Local));
                }
                DisplaysAction::Show { name } => {
                    let entry = client.display(&name).await.map_err(failed)?;
                    println!(
                        "{}: {}",
                        entry.name,
                        describe_entry(&entry, Utc::now(), &Local)
                    );
                }
            }
            Ok(())
        }
        Command::Layout { action } => {
            match action {
                LayoutAction::List => {
                    let list = client.layouts().await.map_err(failed)?;
                    println!("{}", describe_layouts(&list));
                }
                LayoutAction::Set { name, layout } => {
                    let entry = client.choose_layout(&name, &layout).await.map_err(failed)?;
                    println!("{}", layout_changed(&entry));
                }
                LayoutAction::Clear { name } => {
                    let entry = client.clear_layout(&name).await.map_err(failed)?;
                    println!("{}", layout_changed(&entry));
                }
            }
            Ok(())
        }
        Command::Viewer { action } => {
            match action {
                ViewerAction::Create {
                    label,
                    layouts,
                    lifetime_days,
                    refresh,
                    code_minutes,
                    qr,
                    no_qr,
                } => {
                    let issued = client
                        .issue_viewer(&IssueViewer {
                            label,
                            layouts,
                            lifetime_days,
                            code_minutes,
                            refresh,
                        })
                        .await
                        .map_err(failed)?;
                    println!("{}", viewer_issued(&issued, Utc::now(), &Local));
                    if qr || (!no_qr && std::io::stdout().is_terminal()) {
                        match qr_for_terminal(&issued.link) {
                            Ok(code) => println!(
                                "Or scan this with the device's camera, which opens the same link (it works once, too):\n\n{code}"
                            ),
                            // The link is above, so this costs only the convenience.
                            Err(e) => eprintln!(
                                "Could not draw a QR code for the link ({e}); use the link above"
                            ),
                        }
                    }
                }
                ViewerAction::List => {
                    let list = client.viewers().await.map_err(failed)?;
                    println!("{}", describe_viewers(&list, Utc::now(), &Local));
                }
                ViewerAction::Revoke { id } => {
                    let entry = client.revoke_viewer(&id).await.map_err(failed)?;
                    println!("{}", viewer_changed(&entry, false));
                }
                ViewerAction::Forget { id } => {
                    let entry = client.forget_viewer(&id).await.map_err(failed)?;
                    println!("{}", viewer_changed(&entry, true));
                }
            }
            Ok(())
        }
        Command::Credential { action } => {
            match action {
                CredentialAction::List => {
                    let list = client.credentials().await.map_err(failed)?;
                    println!("{}", describe_credentials(&list, Utc::now()));
                }
                CredentialAction::Set { name, remember } => {
                    let key = read_key(&name).map_err(|e| e.to_string())?;
                    let entry = client
                        .set_credential(&name, &key, remember)
                        .await
                        .map_err(failed)?;
                    println!("{}", credential_changed(&entry, false));
                }
                CredentialAction::Clear { name } => {
                    let entry = client.clear_credential(&name).await.map_err(failed)?;
                    println!("{}", credential_changed(&entry, true));
                }
            }
            Ok(())
        }
        Command::Approve { name, code } => {
            let entry = client.approve(&name, &code).await.map_err(failed)?;
            println!("{}", approved(&entry));
            Ok(())
        }
        Command::Reject { name } => {
            let entry = client.reject(&name).await.map_err(failed)?;
            println!("{}", rejected(&entry));
            Ok(())
        }
        Command::Revoke { name } => {
            let entry = client.revoke(&name).await.map_err(failed)?;
            println!("{}", revoked(&entry));
            Ok(())
        }
        Command::Forget { name } => {
            let entry = client.forget(&name).await.map_err(failed)?;
            println!("{}", forgotten(&entry));
            Ok(())
        }
    }
}

/// The key for `name`, from a prompt that does not show what is typed when this is a terminal, and from standard input
/// otherwise. The name is only used to say what the prompt is for.
fn read_key(name: &str) -> std::io::Result<secrecy::SecretString> {
    if std::io::stdin().is_terminal() {
        eprint!("Key for {name} (what you type is not shown): ");
        let typed = rpassword::read_password()?;
        return key_from_stream(typed.as_bytes());
    }
    key_from_stream(std::io::stdin().lock())
}
