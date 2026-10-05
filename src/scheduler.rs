use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Local};
use tokio::sync::Notify;

pub use crate::domain::models::schedule::Schedule;

/// How long an idle Chrome stays connected between periodic renders. Cron gaps can be
/// long (overnight, say), so this is generous; a Chrome that went away anyway is relaunched.
pub const PERIODIC_IDLE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

/// Calls `tick` on the schedule until `shutdown` completes, starting with one run now
/// when `run_now` is set. A failed tick is logged and doesn't stop the loop, and a run
/// that overshoots the next slot skips it instead of running twice in a row.
pub async fn run_periodically<F, Fut>(
    schedule: &Schedule,
    run_now: bool,
    shutdown: impl Future<Output = ()>,
    tick: F,
) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    run_periodically_from(schedule, run_now.then(Local::now), &Notify::new(), shutdown, tick).await
}

/// Like `run_periodically`, but the first run is at `first` (immediately if that is already
/// past), or at the first scheduled slot when it is None. Notifying `wake` runs a tick at once,
/// without moving the schedule.
pub async fn run_periodically_from<F, Fut>(
    schedule: &Schedule,
    first: Option<DateTime<Local>>,
    wake: &Notify,
    shutdown: impl Future<Output = ()>,
    mut tick: F,
) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    tokio::pin!(shutdown);
    let mut due = match first {
        Some(first) => first,
        None => schedule.next_after(Local::now())?,
    };

    loop {
        let wait = (due - Local::now()).to_std().unwrap_or_default();
        log::info!("Next run at {}", due.format("%Y-%m-%d %H:%M:%S"));
        tokio::select! {
            _ = &mut shutdown => {
                log::info!("Shutting down");
                return Ok(());
            }
            _ = tokio::time::sleep(wait) => {}
            _ = wake.notified() => log::info!("Render requested"),
        }

        let started = Local::now();
        // Let a signal interrupt a long fetch or render too.
        tokio::select! {
            _ = &mut shutdown => {
                log::info!("Shutting down mid-run");
                return Ok(());
            }
            result = tick() => {
                if let Err(e) = result {
                    log::error!("Run failed: {e:#}");
                }
            }
        }

        due = schedule.next_after(started)?;
        let finished = Local::now();
        if due <= finished {
            due = schedule.next_after(finished)?;
        }
    }
}

/// Completes on Ctrl-C, or SIGTERM where that exists (systemd stop, docker stop).
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
                return;
            }
            Err(e) => log::warn!("Could not listen for SIGTERM: {e}"),
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use anyhow::anyhow;

    use super::*;

    #[tokio::test]
    async fn runs_now_then_on_schedule_and_survives_failures() {
        let schedule = Schedule::Every(Duration::from_millis(30));
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&runs);

        run_periodically(
            &schedule,
            true,
            tokio::time::sleep(Duration::from_millis(250)),
            move || {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                async move {
                    // The first run fails; the loop must carry on.
                    if n == 0 { Err(anyhow!("boom")) } else { Ok(()) }
                }
            },
        )
        .await
        .unwrap();

        assert!(runs.load(Ordering::SeqCst) >= 3, "ran {} times", runs.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_wake_runs_now_and_leaves_the_schedule_alone() {
        let schedule = Schedule::Every(Duration::from_secs(3600));
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&runs);
        let wake = Arc::new(Notify::new());
        let waker = Arc::clone(&wake);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            waker.notify_one();
        });

        run_periodically_from(&schedule, None, &wake, tokio::time::sleep(Duration::from_millis(300)), move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        })
        .await
        .unwrap();

        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn waits_for_the_first_slot_when_not_running_now() {
        let schedule = Schedule::Every(Duration::from_secs(3600));
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&runs);

        run_periodically(
            &schedule,
            false,
            tokio::time::sleep(Duration::from_millis(50)),
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
        )
        .await
        .unwrap();

        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_slow_run_skips_missed_slots_and_is_interrupted_by_shutdown() {
        let schedule = Schedule::Every(Duration::from_millis(10));
        let started = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&started);

        let begun = std::time::Instant::now();
        run_periodically(
            &schedule,
            true,
            tokio::time::sleep(Duration::from_millis(100)),
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(())
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert!(begun.elapsed() < Duration::from_secs(5));
    }
}
