use anyhow::{bail, Result};
use clap::Parser;
use tracing_subscriber::{fmt, EnvFilter};

use eink_home_display_rust::bootstrap::setup_departure_boards;
use eink_home_display_rust::cli::DeparturesArgs;
use eink_home_display_rust::config::application::ApplicationConfig;
use eink_home_display_rust::domain::models::departures::{DepartureStatus, Departures};

#[tokio::main]
async fn main() -> Result<()> {
    initialize_logging();

    let args = DeparturesArgs::parse();

    let config = match ApplicationConfig::new(&args.config_file) {
        Ok(config) => config,
        Err(e) => {
            log::error!("Failed to load settings: {}", e);
            eprintln!(
                "Error: Failed to load configuration from {}. Please check your config file.",
                args.config_file.display()
            );
            std::process::exit(1);
        }
    };

    let configs: Vec<_> = config
        .departures
        .iter()
        .filter(|b| {
            args.board
                .as_ref()
                .map_or(true, |name| b.name.eq_ignore_ascii_case(name))
        })
        .collect();
    if configs.is_empty() {
        bail!("No matching [[departures]] boards in config");
    }

    for board_config in configs {
        let mut boards = setup_departure_boards(std::slice::from_ref(board_config), &config.providers)?;
        let Some(board) = boards.pop() else {
            println!("\n{}  (disabled)", board_config.name);
            continue;
        };
        let rows = args.rows.unwrap_or(board_config.rows);
        match board.fetch(rows, chrono::Local::now()).await {
            Ok(departures) => print_board(&board_config.name, &departures),
            Err(e) => println!("\n{}\n  error: {e:#}", board_config.name),
        }
    }

    Ok(())
}

fn print_board(name: &str, departures: &Departures) {
    if departures.station.is_empty() {
        println!("\n{name}");
    } else {
        println!("\n{name}  ({})", departures.station);
    }
    if departures.services.is_empty() {
        println!("  no scheduled trains");
        return;
    }
    println!("  {:<8} {:<9} {:<32} IN", "TIME", "EXPECTED", "DESTINATION");
    for service in &departures.services {
        let expected = match service.status {
            DepartureStatus::Cancelled => "cancelled",
            _ => &service.expected,
        };
        println!(
            "  {:<8} {:<9} {:<32} {}",
            service.time, expected, service.destination, service.countdown
        );
    }
}

fn initialize_logging() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));
    fmt().with_env_filter(env_filter).init();
}
