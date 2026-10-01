use std::path::Path;

use clap::Parser;
use eink_home_display_rust::config::application::ApplicationConfig;

/// Generate the config JSON Schema and example TOML from the config types.
#[derive(Parser)]
struct Args {
    /// Write config/schema.json and config/example.toml instead of printing to stdout
    #[arg(long)]
    write: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let schema = ApplicationConfig::json_schema();
    let example = ApplicationConfig::example_toml();

    if args.write {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("config");
        std::fs::write(dir.join("schema.json"), schema)?;
        std::fs::write(dir.join("example.toml"), example)?;
        println!("Wrote config/schema.json and config/example.toml");
    } else {
        println!("{example}");
    }
    Ok(())
}
