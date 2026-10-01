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
    /// Path to application config file. Needed for --live; without it the
    /// Waveshare 7.5" profile is used.
    #[arg(short, long)]
    pub config_file: Option<PathBuf>,

    /// Where to write the rendered PNG; the dithered version goes next to it
    /// with a `.dithered.png` extension
    #[arg(short, long, default_value = "render.png")]
    pub output: PathBuf,

    /// Fetch real weather and departures instead of using sample data
    #[arg(long)]
    pub live: bool,

    /// Also write the rendered HTML (with its fonts) to this directory, to open in a browser
    #[arg(long)]
    pub html_dir: Option<PathBuf>,

    /// How greys are reduced for the dithered preview; defaults to the config's setting
    #[arg(long, value_enum)]
    pub dither: Option<DitherArg>,
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
