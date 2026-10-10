use std::path::Path;

use clap::Parser;
use home_display_server::config::application::ApplicationConfig;

/// Generate the config JSON Schema and example TOML from the config types.
#[derive(Parser)]
struct Args {
    /// Write config/schema.json, config/example.toml, config/openapi.json and config/admin-openapi.json instead
    /// of printing to stdout
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
        std::fs::write(
            dir.join("openapi.json"),
            home_display_server::adapters::image_server::openapi_json(),
        )?;
        std::fs::write(
            dir.join("admin-openapi.json"),
            home_display_server::adapters::admin::openapi_json(),
        )?;
        println!(
            "Wrote config/schema.json, config/example.toml, config/openapi.json and config/admin-openapi.json"
        );
    } else {
        println!("{example}");
    }
    Ok(())
}
