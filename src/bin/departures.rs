use anyhow::Result;
use clap::Parser;
use serde_valid::Validate;
use tracing_subscriber::{fmt, EnvFilter};

use eink_home_display_rust::adapters::departures::setup_departures_service;
use eink_home_display_rust::cli::{DeparturesArgs, Direction};
use eink_home_display_rust::config::application::ApplicationConfig;
use eink_home_display_rust::domain::models::departures::DepartureService;
use eink_home_display_rust::domain::services::departures_service::DeparturesService;

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

    if let Err(e) = config.validate() {
        log::error!("Configuration validation failed: {}", e);
        eprintln!("Error: Configuration is invalid. Please check your config file.");
        std::process::exit(1);
    }

    let service = setup_departures_service(&config.departures);
    let stations = &config.departures.stations;

    if matches!(args.direction, Direction::Northbound | Direction::Both) {
        let services = service
            .get_departures(&stations.northbound_from, &stations.northbound_to, args.rows)
            .await?;
        print_board(
            "NORTHBOUND",
            &stations.northbound_from,
            &stations.northbound_to,
            &services,
        );
    }

    if matches!(args.direction, Direction::Southbound | Direction::Both) {
        let services = service
            .get_departures(&stations.southbound_from, &stations.southbound_to, args.rows)
            .await?;
        print_board(
            "SOUTHBOUND",
            &stations.southbound_from,
            &stations.southbound_to,
            &services,
        );
    }

    Ok(())
}

fn print_board(label: &str, from: &str, to: &str, services: &[DepartureService]) {
    println!("\n{label}  {from} -> {to}");
    if services.is_empty() {
        println!("  no scheduled trains");
        return;
    }
    println!("  {:<6} {:<24} {:<10} DELAY", "TIME", "DESTINATION", "STATUS");
    for service in services {
        println!(
            "  {:<6} {:<24} {:<10} {}",
            service.time, service.destination, service.status, service.delay
        );
    }
}

fn initialize_logging() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));
    fmt().with_env_filter(env_filter).init();
}
