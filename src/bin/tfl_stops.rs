use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt};

use eink_home_display_rust::adapters::stop_points::tfl::tfl_stop_point_service::{
    DEFAULT_HOST_URL, TflStopPointServiceAdapter,
};
use eink_home_display_rust::cli::{StopsCommand, TflStopsArgs};
use eink_home_display_rust::domain::models::arrival::Arrival;
use eink_home_display_rust::domain::models::location::Location;
use eink_home_display_rust::domain::models::stop_point::{StopKind, StopPoint};
use eink_home_display_rust::domain::services::arrivals_service::ArrivalsService;
use eink_home_display_rust::domain::services::stop_point_service::StopPointService;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_logging();

    let args = TflStopsArgs::parse();
    let service = TflStopPointServiceAdapter::new(
        args.host_url.unwrap_or_else(|| DEFAULT_HOST_URL.to_owned()),
        args.app_key.map(Into::into),
        reqwest::Client::new(),
    );

    if let StopsCommand::Arrivals { stop_id, limit } = &args.command {
        let arrivals = service.get_arrivals(stop_id).await?;
        print_arrivals(stop_id, &arrivals[..arrivals.len().min(*limit)]);
        return Ok(());
    }

    let (label, stops) = match args.command {
        StopsCommand::Bus { lat, lon, radius } => (
            "BUS STOPS",
            service
                .find_nearby(Location::new(lat, lon), StopKind::Bus, radius)
                .await?,
        ),
        StopsCommand::Tube { lat, lon, radius } => (
            "UNDERGROUND STATIONS",
            service
                .find_nearby(Location::new(lat, lon), StopKind::Underground, radius)
                .await?,
        ),
        StopsCommand::Arrivals { .. } => unreachable!("handled above"),
        StopsCommand::TubeSearch { query } => (
            "UNDERGROUND STATIONS",
            service
                .search_by_name(&query, StopKind::Underground)
                .await?,
        ),
    };

    print_stops(label, &stops);
    Ok(())
}

fn print_stops(label: &str, stops: &[StopPoint]) {
    println!("\n{label}");
    if stops.is_empty() {
        println!("  none found");
        return;
    }
    println!(
        "  {:<14} {:<30} {:<8} {:>6}  LINES",
        "ID", "NAME", "STOP", "DIST"
    );
    for stop in stops {
        let distance = stop
            .distance
            .map(|d| format!("{d:.0}m"))
            .unwrap_or_default();
        println!(
            "  {:<14} {:<30} {:<8} {:>6}  {}",
            stop.id,
            stop.name,
            stop.indicator.as_deref().unwrap_or(""),
            distance,
            stop.lines.join(", ")
        );
    }
}

fn print_arrivals(stop_id: &str, arrivals: &[Arrival]) {
    println!("\nARRIVALS  {stop_id}");
    if arrivals.is_empty() {
        println!("  none predicted");
        return;
    }
    println!(
        "  {:<11} {:<7} {:<38} {:<26} LOCATION",
        "LINE", "DUE", "TOWARDS", "PLATFORM"
    );
    for a in arrivals {
        let due = match a.seconds_to_arrival {
            0..=29 => "due".to_owned(),
            s => format!("{} min", s / 60),
        };
        println!(
            "  {:<11} {:<7} {:<38} {:<26} {}",
            a.line, due, a.towards, a.platform, a.current_location
        );
    }
}

fn initialize_logging() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    fmt().with_env_filter(env_filter).init();
}
