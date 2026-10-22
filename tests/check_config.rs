//! The real `home-display-server --check-config`, which the installer runs, against the configuration it installs.

use std::path::Path;
use std::process::{Command, Output};

const TEMPLATE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/deploy/config.production.toml");
const STATE: &str = "/var/lib/home-display";

/// The installed template with its folders moved into `state`, and written there as `config.toml`.
fn config_in(state: &Path, edit: impl Fn(String) -> String) -> std::path::PathBuf {
    let text = std::fs::read_to_string(TEMPLATE).unwrap();
    // Forward slashes are a path in a TOML string on Windows as well.
    let moved = text.replace(
        STATE,
        &state
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    );
    let path = state.join("config.toml");
    std::fs::write(&path, edit(moved)).unwrap();
    path
}

fn check(state: &Path, config: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_home-display-server"))
        .current_dir(state)
        .env("RUST_LOG", "warn")
        .arg("--config-file")
        .arg(config)
        .arg("--check-config")
        .args(extra)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn the_configuration_the_installer_makes_is_valid() {
    let state = tempfile::tempdir().unwrap();
    let config = config_in(state.path(), |text| text);
    let output = check(state.path(), &config, &[]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("is valid"), "{}", text(&output));
}

#[test]
fn the_first_install_makes_the_certificate_authority_and_nothing_serves() {
    let state = tempfile::tempdir().unwrap();
    let config = config_in(state.path(), |text| text);
    let output = check(state.path(), &config, &["--init-pki"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(state.path().join("pki/root.key").exists());
    assert!(state.path().join("pki/pairings.json").exists());
}

#[test]
fn https_with_no_authority_is_refused_and_makes_none() {
    // A directory with nothing in it is most often a volume that was not mounted: it is not a fresh start.
    let state = tempfile::tempdir().unwrap();
    let config = config_in(state.path(), |text| {
        text.replace("\"prefer-https\"", "\"https\"")
    });
    let output = check(state.path(), &config, &[]);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(!state.path().join("pki/root.key").exists());
}

#[test]
fn a_schedule_that_is_not_one_is_refused() {
    let state = tempfile::tempdir().unwrap();
    let config = config_in(state.path(), |text| {
        text.replace("*/10 * * * *", "every now and then")
    });
    let output = check(state.path(), &config, &[]);
    assert!(!output.status.success(), "{}", text(&output));
}

#[test]
fn a_configuration_that_is_not_one_is_refused() {
    let state = tempfile::tempdir().unwrap();
    let config = config_in(state.path(), |text| text.replace("[display]", "[display"));
    let output = check(state.path(), &config, &[]);
    assert!(!output.status.success(), "{}", text(&output));
}
