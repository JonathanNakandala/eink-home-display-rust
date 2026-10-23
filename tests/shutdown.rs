//! The real server, stopped while a render is stuck in something that cannot be cancelled.
//!
//! A render starts Chrome, or downloads it, in a blocking call that nothing can interrupt, and takes up to 30 s. A stop used to wait
//! for it to end (the runtime waits for every blocking call when it is dropped), which outlasted the unit's TimeoutStopSec, so
//! systemd killed the program and called the stop a failure. It now goes within a few seconds.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TEMPLATE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/deploy/config.production.toml");
const STATE: &str = "/var/lib/home-display";

/// A Chrome that says its version, as the probe asks, and then never starts: what launching one that hangs looks like. It ends
/// after a while, so that nothing is left behind if the test fails.
const FAKE_CHROME: &str =
    "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'Fake Chrome 1.0'; exit 0; fi\nsleep 25\n";

#[test]
fn a_stop_during_a_render_that_cannot_be_cancelled_ends_the_program_in_seconds() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path();

    // The configuration the installer makes, in a folder of its own, with nothing that needs the network or the machine: no weather,
    // no announcing, plain HTTP, any port.
    let text = std::fs::read_to_string(TEMPLATE)
        .unwrap()
        .replace(STATE, &state.to_string_lossy())
        .replace("[weather]\nenabled = true", "[weather]\nenabled = false")
        .replace("\"prefer-https\"", "\"http\"")
        .replace("bind = \"[::]:8080\"", "bind = \"127.0.0.1:0\"")
        .replace("[server]\n", "[server]\nadvertise = false\n");
    let config = state.join("config.toml");
    std::fs::write(&config, text).unwrap();

    let chrome = state.join("fake-chrome");
    std::fs::write(&chrome, FAKE_CHROME).unwrap();
    std::fs::set_permissions(&chrome, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut server = Command::new(env!("CARGO_BIN_EXE_home-display-server"))
        .current_dir(state)
        .env("RUST_LOG", "info")
        .env("CHROME", &chrome)
        .arg("--config-file")
        .arg(&config)
        // Its own process group, so that whatever it started (the fake Chrome) can be ended with it at the end.
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = i32::try_from(server.id()).unwrap();

    // Wait for the render to be inside the launch of Chrome, which is where it cannot be stopped.
    let (seen, started) = mpsc::channel();
    let stdout = server.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut log = Vec::new();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.contains("Launching Chrome binary") {
                let _ = seen.send(());
            }
            log.push(line);
        }
        log
    });
    let launched = started.recv_timeout(Duration::from_secs(20)).is_ok();

    let stopped_at = Instant::now();
    // SAFETY: kill(2) takes two integers and touches no memory.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let status = loop {
        if let Some(status) = server.try_wait().unwrap() {
            break Some(status);
        }
        if stopped_at.elapsed() > Duration::from_secs(40) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let took = stopped_at.elapsed();

    // Whatever is left, the fake Chrome included, goes with the group.
    // SAFETY: as above; a negative pid is the process group.
    unsafe { libc::kill(-pid, libc::SIGKILL) };
    let _ = server.wait();
    let log = reader.join().unwrap().join("\n");

    assert!(
        launched,
        "the render never got as far as starting Chrome:\n{log}"
    );
    let status =
        status.unwrap_or_else(|| panic!("it was still running 40 s after the stop:\n{log}"));
    assert!(status.success(), "it stopped with {status}:\n{log}");
    // The launch is stuck for 25 s, and the render tries it a second time, so a stop that waits for it is still running after 40 s
    // (as the old one was); one that does not takes the few seconds it allows. Well apart, and the 12 s is the unit's 20 s less a
    // margin for a slow machine.
    assert!(
        took < Duration::from_secs(12),
        "a stop took {took:?}, as long as waiting for the render:\n{log}"
    );
}
