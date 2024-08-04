use std::path::PathBuf;

#[derive(clap::Parser)]
pub struct Args {
    /// Path to application config file
    #[arg(short, long)]
    pub config_file: PathBuf,
}
