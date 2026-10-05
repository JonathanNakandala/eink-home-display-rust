use std::future::Future;
use std::time::Duration;

use anyhow::{anyhow, Context};
use chrono::{DateTime, Local};
use croner::Cron;

/// How long an idle Chrome stays connected between periodic renders. Cron gaps can be
/// long (overnight, say), so this is generous; a Chrome that went away anyway is relaunched.
pub const PERIODIC_IDLE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

/// When the periodic mode should run next.
#[derive(Debug, Clone)]
pub enum Schedule {
    /// Wall-clock aligned, in local time, e.g. `*/10 * * * *`.
    Cron(Cron),
    /// A fixed gap after each run starts.
    Every(Duration),
}

impl Schedule {
    pub fn parse_cron(expression: &str) -> anyhow::Result<Self> {
        let cron = expression
            .parse::<Cron>()
            .map_err(|e| anyhow!("Invalid cron expression {expression:?}: {e}"))?;
        Ok(Self::Cron(cron))
    }

    pub fn parse_every(period: &str) -> anyhow::Result<Self> {
        let period = humantime::parse_duration(period)
            .with_context(|| format!("Invalid interval {period:?}, expected e.g. 10m or 90s"))?;
        anyhow::ensure!(!period.is_zero(), "The interval must be longer than zero");
        Ok(Self::Every(period))
    }

    /// The first time strictly after `after` at which a run is due.
    pub fn next_after(&self, after: DateTime<Local>) -> anyhow::Result<DateTime<Local>> {
        match self {
            Self::Cron(cron) => cron
                .find_next_occurrence(&after, false)
                .map_err(|e| anyhow!("No next run time: {e}")),
            Self::Every(period) => Ok(after + *period),
        }
    }
}

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
    run_periodically_from(schedule, run_now.then(Local::now), shutdown, tick).await
}

/// Like `run_periodically`, but the first run is at `first` (immediately if that is already
/// past), or at the first scheduled slot when it is None.
pub async fn run_periodically_from<F, Fut>(
    schedule: &Schedule,
    first: Option<DateTime<Local>>,
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

    use chrono::TimeZone;

    use super::*;

    fn at(h: u32, m: u32, s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 6, 15, h, m, s).unwrap()
    }

    #[test]
    fn cron_aligns_to_the_clock() {
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        assert_eq!(schedule.next_after(at(8, 3, 20)).unwrap(), at(8, 10, 0));
        // Strictly after: a run starting on the slot doesn't repeat it.
        assert_eq!(schedule.next_after(at(8, 10, 0)).unwrap(), at(8, 20, 0));
    }

    #[test]
    fn every_adds_the_period() {
        let schedule = Schedule::parse_every("90s").unwrap();
        assert_eq!(schedule.next_after(at(8, 0, 0)).unwrap(), at(8, 1, 30));
    }

    #[test]
    fn rejects_bad_schedules() {
        assert!(Schedule::parse_cron("not a cron").is_err());
        assert!(Schedule::parse_every("soon").is_err());
        assert!(Schedule::parse_every("0s").is_err());
    }

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
