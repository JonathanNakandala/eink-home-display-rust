use std::path::PathBuf;

use crate::adapters::display_image_generator::chrome_render::ChromeSource;
use crate::scheduler::Schedule;

#[derive(clap::Parser)]
pub struct Args {
    /// Path to application config file
    #[arg(short, long)]
    pub config_file: PathBuf,

    /// Cache directory, overriding cache.directory from the config
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Always render with the pinned Chrome downloaded into the cache, even if one is installed
    #[arg(long)]
    pub bundled_chrome: bool,

    /// Keep running and refresh on a cron schedule in local time, e.g. "*/10 * * * *"
    #[arg(long, value_parser = Schedule::parse_cron, conflicts_with = "every")]
    pub cron: Option<Schedule>,

    /// Keep running and refresh at this interval, e.g. 10m or 90s
    #[arg(long, value_parser = Schedule::parse_every)]
    pub every: Option<Schedule>,

    /// With --cron or --every, wait for the first slot instead of refreshing at startup
    #[arg(long)]
    pub no_initial_run: bool,
}

impl From<bool> for ChromeSource {
    /// From a `--bundled-chrome` flag.
    fn from(bundled: bool) -> Self {
        if bundled { Self::Bundled } else { Self::PreferSystem }
    }
}

impl Args {
    /// The schedule to run on, or None for a single run.
    pub fn schedule(&self) -> Option<&Schedule> {
        self.cron.as_ref().or(self.every.as_ref())
    }
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

#[derive(clap::Parser)]
pub struct DeparturesArgs {
    /// Path to application config file
    #[arg(short, long)]
    pub config_file: PathBuf,

    /// Only print the board with this name (case-insensitive); default is all
    #[arg(long)]
    pub board: Option<String>,

    /// Override the number of services requested per board
    #[arg(long)]
    pub rows: Option<u8>,
}

#[derive(clap::Parser)]
pub struct TflStopsArgs {
    /// TfL API app key (optional; anonymous requests are rate limited)
    #[arg(long, env = "TFL_APP_KEY", global = true)]
    pub app_key: Option<String>,

    /// Override the TfL API base URL
    #[arg(long, global = true)]
    pub host_url: Option<String>,

    #[command(subcommand)]
    pub command: StopsCommand,
}

#[derive(clap::Subcommand)]
pub enum StopsCommand {
    /// Find bus stops near a point
    Bus {
        #[arg(long, allow_hyphen_values = true)]
        lat: f64,
        #[arg(long, allow_hyphen_values = true)]
        lon: f64,
        /// Search radius in metres
        #[arg(long, default_value_t = 200)]
        radius: u32,
    },
    /// Find Underground stations near a point
    Tube {
        #[arg(long, allow_hyphen_values = true)]
        lat: f64,
        #[arg(long, allow_hyphen_values = true)]
        lon: f64,
        /// Search radius in metres
        #[arg(long, default_value_t = 1000)]
        radius: u32,
    },
    /// Show predicted arrivals at a bus stop or station, by stop ID
    /// (e.g. 940GZZLUTPN for Turnpike Lane, 490000173RC for a bus stop)
    Arrivals {
        stop_id: String,
        /// Maximum number of arrivals to show
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Search Underground stations by name
    TubeSearch {
        /// Station name, e.g. "finsbury park"
        query: String,
    },
}

#[derive(clap::Parser)]
pub struct RenderArgs {
    /// Path to application config file. Needed for --live.
    #[arg(short, long)]
    pub config_file: Option<PathBuf>,

    /// Cache directory, overriding cache.directory from the config; default is ./cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Always render with the pinned Chrome downloaded into the cache, even if one is installed
    #[arg(long)]
    pub bundled_chrome: bool,

    /// Folder for the rendered images, created if missing
    #[arg(short, long, default_value = "output")]
    pub output_dir: PathBuf,

    /// Display(s) to render for; repeat for several. Defaults to all of them.
    #[arg(short, long, value_enum)]
    pub display: Vec<DisplayArg>,

    /// Fetch real weather and departures instead of using sample data
    #[arg(long)]
    pub live: bool,

    /// Also write each display's HTML and fonts to <output-dir>/page_<display>/, to open in a browser
    #[arg(long)]
    pub html: bool,

    /// How greys are reduced for the preview; defaults to the config's setting, else none
    #[arg(long, value_enum)]
    pub dither: Option<DitherArg>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum DisplayArg {
    #[value(name = "waveshare")]
    Waveshare,
    #[value(name = "reterminal-e1003", alias = "reterminal")]
    ReTerminalE1003,
}

impl From<DisplayArg> for crate::config::application::DisplayKind {
    fn from(arg: DisplayArg) -> Self {
        match arg {
            DisplayArg::Waveshare => Self::WaveshareEpd7in5V2,
            DisplayArg::ReTerminalE1003 => Self::ReTerminalE1003,
        }
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum DitherArg {
    None,
    FloydSteinberg,
    Ordered,
}

impl From<DitherArg> for crate::domain::models::display::Dither {
    fn from(arg: DitherArg) -> Self {
        match arg {
            DitherArg::None => Self::None,
            DitherArg::FloydSteinberg => Self::FloydSteinberg,
            DitherArg::Ordered => Self::Ordered,
        }
    }
}
