use anyhow::{bail, Result};
use clap::Parser;

use eink_home_display_rust::bootstrap::{self, setup_departure_boards};
use eink_home_display_rust::cli::DeparturesArgs;
use eink_home_display_rust::domain::models::departures::{DepartureStatus, Departures};

#[tokio::main]
async fn main() -> Result<()> {
    bootstrap::init_logging("debug");

    let args = DeparturesArgs::parse();

    let config = bootstrap::load_application_config(&args.config_file)?;

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
