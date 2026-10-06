use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Local};
use tokio::sync::Notify;

pub use crate::domain::models::schedule::Schedule;
use crate::domain::models::freshness::format_age;
use crate::domain::services::clock::Clock;

/// How long an idle Chrome stays connected between periodic renders. Cron gaps can be
/// long (overnight, say), so this is generous; a Chrome that went away anyway is relaunched.
pub const PERIODIC_IDLE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

/// What the schedule means, for a person to check against what they meant: each expression in words,
/// how many refreshes each of the next days gets (the number that decides battery life), and the next few.
pub fn schedule_summary(schedule: &Schedule, now: DateTime<Local>) -> Vec<String> {
    let mut lines = vec!["Schedule:".to_owned()];
    lines.extend(schedule.describe().into_iter().map(|line| format!("  {line}")));
    let days: Vec<String> = (0..7)
        .filter_map(|offset| now.date_naive().checked_add_days(chrono::Days::new(offset)))
        .map(|day| match schedule.runs_on(day) {
            Ok(runs) => format!("{} {runs}", day.format("%a")),
            Err(e) => format!("{} ({e:#})", day.format("%a")),
        })
        .collect();
    lines.push(format!("Refreshes per day: {}", days.join(", ")));
    match schedule.upcoming(now, 3) {
        Ok(runs) => {
            let times: Vec<String> = runs.iter().map(|at| at.format("%a %H:%M:%S").to_string()).collect();
            lines.push(format!("Next refreshes: {}", times.join(", ")));
        }
        Err(e) => lines.push(format!("No next refresh: {e:#}")),
    }
    lines
}

pub fn log_schedule(schedule: &Schedule, now: DateTime<Local>) {
    for line in schedule_summary(schedule, now) {
        log::info!("{line}");
    }
}

/// How often the loop looks at the clock while it waits for the next run. A wait is worked out from the
/// clock once, so a clock that is then set (NTP just after boot, a manual correction) would otherwise
/// leave the loop sleeping for a time that no longer means anything.
const CLOCK_CHECK: Duration = Duration::from_secs(30);

/// A clock that reads this much earlier than the last look has been set back, not just read twice.
const STEP_BACK: Duration = Duration::from_secs(2);

/// Calls `tick` on the schedule until `shutdown` completes, starting with one run now
/// when `run_now` is set. A failed tick is logged and doesn't stop the loop, and a run
/// that overshoots the next slot skips it instead of running twice in a row.
pub async fn run_periodically<F, Fut>(
    schedule: &Schedule,
    run_now: bool,
    clock: &dyn Clock,
    shutdown: impl Future<Output = ()>,
    tick: F,
) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    run_periodically_from(schedule, run_now.then(|| clock.now()), &Notify::new(), clock, shutdown, tick).await
}

/// Like `run_periodically`, but the first run is at `first` (immediately if that is already
/// past), or at the first scheduled slot when it is None. Notifying `wake` runs a tick at once,
/// without moving the schedule.
///
/// The wait for each run is taken in short slices, looking at `clock` between them, so a clock that
/// changes meanwhile is noticed. Set forward, a run that has become overdue happens at the next look.
/// Set back, the next run is planned again from the new time: kept, it would be hours or days away.
pub async fn run_periodically_from<F, Fut>(
    schedule: &Schedule,
    first: Option<DateTime<Local>>,
    wake: &Notify,
    clock: &dyn Clock,
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
        None => schedule.next_after(clock.now())?,
    };

    loop {
        log::info!("Next run at {}", due.format("%Y-%m-%d %H:%M:%S"));
        let mut last_look = clock.now();
        loop {
            let now = clock.now();
            if last_look - now > chrono::Duration::from_std(STEP_BACK)? {
                due = schedule.next_after(now)?;
                log::warn!(
                    "The clock was set back {}: planning the next run again, for {}",
                    format_age(last_look - now),
                    due.format("%Y-%m-%d %H:%M:%S")
                );
            }
            last_look = now;
            let remaining = (due - now).to_std().unwrap_or_default();
            if remaining.is_zero() {
                break;
            }
            tokio::select! {
                _ = &mut shutdown => {
                    log::info!("Shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(remaining.min(CLOCK_CHECK)) => {}
                _ = wake.notified() => {
                    log::info!("Render requested");
                    break;
                }
            }
        }

        let started = clock.now();
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

        let finished = clock.now();
        // From whichever was earlier, so a clock set back during the run doesn't leave the next one far off.
        due = schedule.next_after(started.min(finished))?;
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

    #[test]
    fn the_summary_says_what_the_schedule_does() {
        use chrono::TimeZone;
        let schedule = Schedule::parse_crons(["* 7-8 * * 1-5", "0 8-21 * * 6,0"]).unwrap();
        // Monday 2026-06-15, 06:30.
        let lines = schedule_summary(&schedule, Local.with_ymd_and_hms(2026, 6, 15, 6, 30, 0).unwrap());
        assert_eq!(
            lines,
            [
                "Schedule:",
                "  * 7-8 * * 1-5  Every minute past hour 7 and 8, on Monday, Tuesday, Wednesday, Thursday, and Friday.",
                "  0 8-21 * * 6,0  At minute 0, of hour 8-21, on Sunday and Saturday.",
                "Refreshes per day: Mon 120, Tue 120, Wed 120, Thu 120, Fri 120, Sat 14, Sun 14",
                "Next refreshes: Mon 07:00:00, Mon 07:01:00, Mon 07:02:00",
            ]
        );
    }

    use std::sync::Arc;

    use std::sync::Mutex;

    use anyhow::anyhow;
    use chrono::TimeZone;

    use super::*;
    use crate::adapters::clock::SystemClock;

    #[tokio::test]
    async fn runs_now_then_on_schedule_and_survives_failures() {
        let schedule = Schedule::Every(Duration::from_millis(30));
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&runs);

        run_periodically(
            &schedule,
            true,
            &SystemClock,
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

        run_periodically_from(&schedule, None, &wake, &SystemClock, tokio::time::sleep(Duration::from_millis(300)), move || {
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
            &SystemClock,
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
            &SystemClock,
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

    fn at(h: u32, m: u32, s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 6, 15, h, m, s).unwrap()
    }

    /// A clock that follows tokio's (pausable) time and can be set forward or back, as NTP would.
    struct SteppedClock {
        start: DateTime<Local>,
        origin: tokio::time::Instant,
        offset: Mutex<chrono::Duration>,
    }

    impl SteppedClock {
        fn new(start: DateTime<Local>) -> Self {
            Self { start, origin: tokio::time::Instant::now(), offset: Mutex::new(chrono::Duration::zero()) }
        }

        fn step(&self, by: chrono::Duration) {
            *self.offset.lock().unwrap() += by;
        }
    }

    impl Clock for SteppedClock {
        fn now(&self) -> DateTime<Local> {
            self.start + chrono::Duration::from_std(self.origin.elapsed()).unwrap() + *self.offset.lock().unwrap()
        }
    }

    /// Runs hourly from 08:00 for three and a half hours of (virtual) time, with the clock stepped by `by` ten minutes
    /// in. Returns how many minutes after the start each run happened.
    async fn runs_with_a_step(by: chrono::Duration) -> Vec<u64> {
        let clock = SteppedClock::new(at(8, 0, 0));
        let started = tokio::time::Instant::now();
        let runs = Mutex::new(Vec::new());
        let schedule = Schedule::parse_every("1h").unwrap();
        let wake = Notify::new();

        let stepper = async {
            tokio::time::sleep(Duration::from_secs(10 * 60)).await;
            clock.step(by);
        };
        let scheduler = run_periodically_from(
            &schedule,
            None,
            &wake,
            &clock,
            tokio::time::sleep(Duration::from_secs(3 * 3600 + 30 * 60)),
            || {
                runs.lock().unwrap().push(started.elapsed().as_secs() / 60);
                async { Ok(()) }
            },
        );
        let (result, ()) = tokio::join!(scheduler, stepper);
        result.unwrap();
        runs.into_inner().unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_step_runs_come_hourly() {
        assert_eq!(runs_with_a_step(chrono::Duration::zero()).await, [60, 120, 180]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_clock_set_back_plans_the_next_run_again_instead_of_waiting_for_the_old_time() {
        // Five hours back: kept, the 09:00 run would now be nearly six hours away and nothing would happen.
        let runs = runs_with_a_step(chrono::Duration::hours(-5)).await;
        // Planned from 03:10 on the new clock: an hour on, give or take the look interval.
        assert_eq!(runs.len(), 3, "{runs:?}");
        assert!((70..=71).contains(&runs[0]), "{runs:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_clock_set_forward_runs_the_overdue_run_at_the_next_look() {
        let runs = runs_with_a_step(chrono::Duration::hours(5)).await;
        // Not at 60 minutes: ten minutes in the clock jumped past 09:00, so it runs within a look.
        assert!((10..=11).contains(&runs[0]), "{runs:?}");
    }
}
