//! What the real `home-display-server` does with its command line, which is what the installer and the release's smoke test try it by.

use std::process::Command;

fn server(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_home-display-server"))
        .args(arguments)
        .output()
        .unwrap()
}

fn text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn help_is_the_usage_and_not_an_error() {
    let output = server(&["--help"]);
    assert!(output.status.success(), "{}", text(&output));
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("Usage:") && out.contains("--config-file") && out.contains("--check-config"),
        "{out}"
    );
}

#[test]
fn version_says_which_release_this_is() {
    let output = server(&["--version"]);
    assert!(output.status.success(), "{}", text(&output));
    let out = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        out.trim(),
        format!("home-display-server {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_mistake_is_said_and_is_not_success() {
    let output = server(&["--no-such-flag"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(
        text(&output).contains("--no-such-flag"),
        "the flag it did not understand is named: {}",
        text(&output)
    );
}
