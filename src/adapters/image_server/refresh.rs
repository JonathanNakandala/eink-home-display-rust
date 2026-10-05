//! Lets a display ask for a render now (its button), instead of waiting for the schedule.
//!
//! The scheduler loop waits on `wake()` as well as the clock, and reports each render's
//! start and end here. A request is refused while a render was started recently, so a held
//! or repeated button press can't turn into a stream of API calls.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{watch, Notify};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// A render ran (or was already running) and has finished.
    Rendered,
    /// A render was started too recently; nothing was triggered.
    Throttled,
    /// The render didn't finish within the time allowed.
    TimedOut,
}

pub struct RefreshControl {
    wake: Notify,
    finished: watch::Sender<u64>,
    last_started: Mutex<Option<Instant>>,
    cooldown: Duration,
}

impl RefreshControl {
    pub fn new(cooldown: Duration) -> Arc<Self> {
        Arc::new(Self { wake: Notify::new(), finished: watch::channel(0).0, last_started: Mutex::new(None), cooldown })
    }

    /// Completes when a render has been requested; the scheduler loop selects on it.
    pub fn wake(&self) -> &Notify {
        &self.wake
    }

    pub fn render_started(&self) {
        *self.last_started.lock().unwrap() = Some(Instant::now());
    }

    /// Called when a render ends, whether or not it worked.
    pub fn render_finished(&self) {
        self.finished.send_modify(|count| *count += 1);
    }

    /// Triggers a render and waits for it, unless one started within the cooldown.
    pub async fn request(&self, timeout: Duration) -> RefreshOutcome {
        {
            let mut last = self.last_started.lock().unwrap();
            let now = Instant::now();
            if last.is_some_and(|started| now.duration_since(started) < self.cooldown) {
                return RefreshOutcome::Throttled;
            }
            // Reserved now, so a second request arriving before the loop wakes is throttled too.
            *last = Some(now);
        }
        let mut finished = self.finished.subscribe();
        let seen = *finished.borrow();
        self.wake.notify_one();
        match tokio::time::timeout(timeout, finished.wait_for(|count| *count > seen)).await {
            Ok(Ok(_)) => RefreshOutcome::Rendered,
            _ => RefreshOutcome::TimedOut,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays the scheduler loop: renders (taking `work`) each time it is woken.
    fn spawn_loop(control: &Arc<RefreshControl>, work: Duration) -> Arc<std::sync::atomic::AtomicUsize> {
        let renders = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (control, count) = (Arc::clone(control), Arc::clone(&renders));
        tokio::spawn(async move {
            loop {
                control.wake().notified().await;
                control.render_started();
                tokio::time::sleep(work).await;
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                control.render_finished();
            }
        });
        renders
    }

    #[tokio::test]
    async fn a_request_renders_and_waits_for_it() {
        let control = RefreshControl::new(Duration::from_secs(30));
        let renders = spawn_loop(&control, Duration::from_millis(50));

        assert_eq!(control.request(Duration::from_secs(5)).await, RefreshOutcome::Rendered);
        assert_eq!(renders.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_second_request_inside_the_cooldown_is_throttled() {
        let control = RefreshControl::new(Duration::from_secs(30));
        let renders = spawn_loop(&control, Duration::from_millis(10));

        assert_eq!(control.request(Duration::from_secs(5)).await, RefreshOutcome::Rendered);
        assert_eq!(control.request(Duration::from_secs(5)).await, RefreshOutcome::Throttled);
        assert_eq!(renders.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn requests_arriving_together_cause_one_render() {
        let control = RefreshControl::new(Duration::from_secs(30));
        let renders = spawn_loop(&control, Duration::from_millis(50));

        let (a, b) = tokio::join!(control.request(Duration::from_secs(5)), control.request(Duration::from_secs(5)));
        let mut outcomes = [a, b];
        outcomes.sort_by_key(|o| *o as u8);
        assert_eq!(outcomes, [RefreshOutcome::Rendered, RefreshOutcome::Throttled]);
        assert_eq!(renders.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_render_that_never_finishes_times_out() {
        let control = RefreshControl::new(Duration::ZERO);
        assert_eq!(control.request(Duration::from_millis(50)).await, RefreshOutcome::TimedOut);
    }

    #[tokio::test]
    async fn after_the_cooldown_a_new_request_renders_again() {
        let control = RefreshControl::new(Duration::from_millis(40));
        let renders = spawn_loop(&control, Duration::from_millis(5));

        assert_eq!(control.request(Duration::from_secs(5)).await, RefreshOutcome::Rendered);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(control.request(Duration::from_secs(5)).await, RefreshOutcome::Rendered);
        assert_eq!(renders.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
