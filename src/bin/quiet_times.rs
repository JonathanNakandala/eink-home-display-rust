use anyhow::Context;
use chrono::{Duration, NaiveDate, NaiveTime};
use clap::Parser;

use eink_home_display_rust::adapters::train_schedule::network_rail::cif_feed;
use eink_home_display_rust::adapters::train_schedule::network_rail::network_rail_train_schedule_service::NetworkRailTrainScheduleServiceAdapter;
use eink_home_display_rust::adapters::train_schedule::network_rail::schedule_cache;
use eink_home_display_rust::bootstrap;
use eink_home_display_rust::cli::QuietTimesArgs;
use eink_home_display_rust::config::quiet_times::QuietTimesRulesConfig;
use eink_home_display_rust::domain::services::quiet_times_calculator::{
    Gap, QuietTimesCalculator, QuietTimesReport, QuietTimesRules, TimeWindow,
};
use eink_home_display_rust::quiet_times::QuietTimesApplication;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap::init_logging("debug");

    let args = QuietTimesArgs::parse();

    let config = bootstrap::load_quiet_times_config(&args.config_file)?;

    let network_rail = &config.network_rail;
    if !network_rail.enabled {
        println!("Network Rail quiet-times feature disabled via config.");
        return Ok(());
    }

    let raw_file = args.input_file.as_ref().unwrap_or(&network_rail.raw_file);

    if let Some(term) = &args.find {
        for (code, description) in cif_feed::find_tiplocs(raw_file, term)? {
            println!("{:8} {}", code, description.unwrap_or_default());
        }
        return Ok(());
    }

    let client = reqwest::Client::new();

    if args.download {
        println!("Downloading full schedule (large file, may take a few minutes)...");
        cif_feed::download_full_schedule(
            &client,
            &network_rail.feed_url,
            &network_rail.username,
            &network_rail.password,
            &network_rail.raw_file,
        )
        .await
        .context("Failed to download CIF schedule feed")?;
        return Ok(());
    }

    if args.build {
        println!("Extracting trains near configured timing points...");
        let lines = cif_feed::open_raw_lines(raw_file)?.filter_map(Result::ok);
        let records = schedule_cache::build_cache_records(lines, &network_rail.timing_points)?;
        schedule_cache::save_cache(&network_rail.cache_file, &records)?;
        return Ok(());
    }

    let start = match &args.start {
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d").context("Invalid --start date")?,
        None => chrono::Utc::now().with_timezone(&eink_home_display_rust::adapters::zone::host()).date_naive(),
    };

    let app = QuietTimesApplication::new(
        NetworkRailTrainScheduleServiceAdapter::new(network_rail.cache_file.clone()),
        QuietTimesCalculator::new(build_rules(&config.rules)?),
    );

    // Only affects which gaps are worth listing individually; the day's single
    // largest gap is always reported regardless of this threshold.
    let min_gap = Duration::minutes(config.rules.min_gap_minutes);

    let report = app.run(start, args.days).await?;
    print_chronological(&report, min_gap);
    let longest: Vec<_> = report
        .longest_gaps(usize::MAX)
        .into_iter()
        .filter(|(_, gap)| gap.duration() >= min_gap)
        .take(10)
        .collect();
    print_longest_gaps_table(&longest);
    print_top_gaps_per_day(&report, args.top_per_day);

    Ok(())
}

fn build_rules(config: &QuietTimesRulesConfig) -> anyhow::Result<QuietTimesRules> {
    Ok(QuietTimesRules::new(
        Duration::minutes(config.buffer_minutes),
        parse_window(&config.weekday_window)?,
        parse_window(&config.weekend_window)?,
        config.use_time_windows,
    ))
}

fn parse_window((start, end): &(String, String)) -> anyhow::Result<TimeWindow> {
    Ok(TimeWindow {
        start: NaiveTime::parse_from_str(start, "%H:%M").context("Invalid window start time")?,
        end: NaiveTime::parse_from_str(end, "%H:%M").context("Invalid window end time")?,
    })
}

fn print_chronological(report: &QuietTimesReport, min_gap: Duration) {
    for day in &report.days {
        println!(
            "\n{}  {}-{}  trains: {}",
            day.date.format("%a %d %b"),
            day.window_start.format("%H:%M"),
            day.window_end.format("%H:%M"),
            day.train_count
        );
        let notable_gaps: Vec<_> = day.gaps.iter().filter(|g| g.duration() >= min_gap).collect();
        if notable_gaps.is_empty() {
            println!("  no gaps of {} min or more", min_gap.num_minutes());
        }
        for gap in notable_gaps {
            println!("  {}", format_gap(gap));
        }
        if let Some(largest) = day.largest_gap() {
            println!("  largest gap: {}", format_gap(largest));
        }
    }
}

fn print_longest_gaps_table(gaps: &[(NaiveDate, Gap)]) {
    println!("\nLongest gaps:");
    if gaps.is_empty() {
        println!("  none found");
    }
    for (date, gap) in gaps {
        println!("  {}  {}", date.format("%a %d %b"), format_gap(gap));
    }
}

fn print_top_gaps_per_day(report: &QuietTimesReport, limit: usize) {
    println!("\nTop {} gaps per day:", limit);
    for day in &report.days {
        println!("\n{}", day.date.format("%a %d %b"));
        let top = day.longest_gaps(limit);
        if top.is_empty() {
            println!("  no gaps found");
        }
        for gap in top {
            println!("  {}", format_gap(gap));
        }
    }
}

fn format_gap(gap: &Gap) -> String {
    format!(
        "{} - {}  ({} min)",
        gap.start.format("%H:%M"),
        gap.end.format("%H:%M"),
        gap.duration().num_minutes()
    )
}
