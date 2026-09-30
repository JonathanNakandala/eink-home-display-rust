use std::path::PathBuf;

#[derive(clap::Parser)]
pub struct Args {
    /// Path to application config file
    #[arg(short, long)]
    pub config_file: PathBuf,
}

#[derive(clap::Parser)]
pub struct QuietTimesArgs {
    /// Path to quiet-times config file
    #[arg(short, long)]
    pub config_file: PathBuf,

    /// Download the full CIF SCHEDULE feed from Network Rail
    #[arg(long)]
    pub download: bool,

    /// Search TIPLOC codes/descriptions in the raw feed, to help configure timing_points
    #[arg(long)]
    pub find: Option<String>,

    /// Rebuild the filtered schedule cache from the raw feed
    #[arg(long)]
    pub build: bool,

    /// Raw CIF SCHEDULE feed file to use for --find/--build, overriding
    /// network_rail.raw_file from the config (e.g. a manually downloaded feed)
    #[arg(long)]
    pub input_file: Option<PathBuf>,

    /// Number of days to report on
    #[arg(long, default_value_t = 14)]
    pub days: u32,

    /// How many of each day's longest gaps to list in the per-day top-gaps table
    #[arg(long, default_value_t = 20)]
    pub top_per_day: usize,

    /// Report start date (YYYY-MM-DD), defaults to today
    #[arg(long)]
    pub start: Option<String>,
}
