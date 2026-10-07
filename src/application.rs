pub mod devices;
pub mod launch;
pub mod plan;
pub mod refresh;
pub mod status;

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use anyhow::anyhow;
use futures_util::FutureExt;
use chrono::{DateTime, Duration};
use chrono_tz::Tz;

use std::future::Future;

use crate::domain::models::departures::Departures;
use crate::domain::models::freshness::{Fetched, LastGood};
use crate::domain::models::location::Location;
use crate::domain::models::render_report::{RenderReport, SourceReport, SourceState};
use crate::domain::models::source_error::SourceError;
use crate::domain::models::weather::WeatherInformation;
use crate::domain::models::{DateInfo, DepartureBoardData, GlanceData};
use crate::domain::services::clock::Clock;
use crate::domain::services::departures_service::DeparturesService;
use crate::domain::services::display_image_generator::DisplayImageGenerator;
use crate::domain::services::image_repository::ImageRepository;
use crate::domain::services::render_observer::RenderObserver;
use crate::domain::services::weather_service::WeatherService;
use crate::domain::services::ImageDisplayService;

/// How old data from an earlier fetch may be when its source fails, before it is dropped
/// from the display as unavailable.
#[derive(Debug, Clone, Copy)]
pub struct MaxAge {
    pub departures: Duration,
    pub weather: Duration,
}

impl Default for MaxAge {
    fn default() -> Self {
        Self { departures: Duration::minutes(15), weather: Duration::minutes(180) }
    }
}

/// How long a run, and each part of it, may take.
#[derive(Debug, Clone, Copy)]
pub struct RenderLimits {
    /// For one source (the weather, or one board) to answer. A source that is too slow counts as
    /// failed, so it is shown from its last good data or as unavailable instead of holding up the rest.
    pub source_timeout: StdDuration,
    /// For the whole run, from fetching to the display. Whatever is still going then is abandoned,
    /// so a stuck render can't stop the next one or a waiting button press.
    pub deadline: StdDuration,
}

impl Default for RenderLimits {
    fn default() -> Self {
        Self { source_timeout: StdDuration::from_secs(40), deadline: StdDuration::from_secs(120) }
    }
}

/// A source's answer, or `SourceError::Timeout` if it takes longer than `limit`.
async fn within<T>(
    limit: StdDuration,
    work: impl Future<Output = Result<T, SourceError>>,
) -> Result<T, SourceError> {
    tokio::time::timeout(limit, work).await.unwrap_or(Err(SourceError::Timeout))
}

/// What a panic said, if it said anything readable (`panic!("...")` carries a `&str` or a `String`).
fn panic_message(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_owned())
}

fn age_seconds(age: Duration) -> u64 {
    age.num_seconds().max(0).unsigned_abs()
}

/// A titled list of departures from one configured source.
#[derive(derive_new::new)]
pub struct DepartureBoard<DS: DeparturesService> {
    name: String,
    rows: u8,
    service: DS,
    /// The last departures this board fetched, for when the source is down.
    #[new(default)]
    last_good: LastGood<Departures>,
}

impl<DS: DeparturesService> DepartureBoard<DS> {
    /// The source's answer as it is, or its error.
    pub async fn fetch(&self, rows: u8, now: DateTime<Tz>) -> Result<Departures, SourceError> {
        self.service.get_departures(rows, now).await
    }

    /// The board as it should be shown: fresh if the source answered, otherwise what it last
    /// said (brought up to date, and labelled with its age) if that is recent enough.
    async fn for_display(
        &self,
        now: DateTime<Tz>,
        max_age: Duration,
        timeout: StdDuration,
    ) -> (DepartureBoardData, SourceReport) {
        let source = format!("Departures for {}", self.name);
        let result = within(timeout, self.fetch(self.rows, now)).await;
        let (board, state) = match self.last_good.resolve(result, now, max_age, &source) {
            Fetched::Fresh(departures) => (
                DepartureBoardData::new(self.name.clone(), departures.station, departures.services),
                SourceState::Fresh,
            ),
            Fetched::Stale { value, age } => {
                let departures = value.as_of(now);
                (
                    DepartureBoardData::from_earlier(self.name.clone(), departures.station, departures.services, age),
                    SourceState::Stale { age_seconds: age_seconds(age) },
                )
            }
            Fetched::Unavailable { reason } => (
                DepartureBoardData::unavailable(self.name.clone(), reason),
                SourceState::Unavailable { reason: reason.to_owned() },
            ),
        };
        (board, SourceReport { name: self.name.clone(), state })
    }
}

#[derive(derive_new::new)]
pub struct Application<WS, DIG, IDS, IR, DS>
where
    WS: WeatherService,
    DIG: DisplayImageGenerator,
    IDS: ImageDisplayService,
    IR: ImageRepository,
    DS: DeparturesService,
{
    weather_service: WS,
    display_image_generator: DIG,
    image_viewing_service: IDS,
    image_repository: IR,
    departure_boards: Vec<DepartureBoard<DS>>,
    max_age: MaxAge,
    limits: RenderLimits,
    clock: Arc<dyn Clock>,
    #[new(default)]
    last_weather: LastGood<Option<WeatherInformation>>,
    /// Told how each run goes; see `with_observer`.
    #[new(default)]
    observers: Vec<Arc<dyn RenderObserver>>,
}

impl<WS, DIG, IDS, IR, DS> Application<WS, DIG, IDS, IR, DS>
where
    WS: WeatherService,
    DIG: DisplayImageGenerator,
    IDS: ImageDisplayService,
    IR: ImageRepository,
    DS: DeparturesService,
{
    /// Has `observer` told when each run starts and how it ends. Observers are told in the order
    /// they were added, so one that releases a waiter should be added after the ones the waiter
    /// will go on to read.
    pub fn with_observer(mut self, observer: Arc<dyn RenderObserver>) -> Self {
        self.observers.push(observer);
        self
    }

    /// Renders and shows the dashboard. A source that fails or is too slow doesn't stop it: that part
    /// is shown from its last good data if recent enough (labelled with the age), or as unavailable.
    /// The whole run is abandoned, with an error, if it takes longer than `limits.deadline`.
    /// On success, says how healthy each source was. The observers hear of the outcome either way.
    ///
    /// A panic in a source, the renderer or the display is caught and reported as a failed run, like
    /// any other: the service also serves the image and its health, and one bad response must not end it.
    pub async fn run(&self, location: Location) -> anyhow::Result<RenderReport> {
        for observer in &self.observers {
            observer.render_started();
        }
        let deadline = self.limits.deadline;
        let result = match AssertUnwindSafe(tokio::time::timeout(deadline, self.render(location))).catch_unwind().await {
            Ok(Ok(rendered)) => rendered,
            Ok(Err(_)) => Err(anyhow!("The render didn't answer within {deadline:?}")),
            Err(panic) => Err(anyhow!("The render panicked: {}", panic_message(panic.as_ref()))),
        };
        let finished = self.clock.now();
        for observer in &self.observers {
            match &result {
                Ok(report) => observer.render_succeeded(finished, report),
                Err(error) => observer.render_failed(finished, error),
            }
        }
        result
    }

    async fn render(&self, location: Location) -> anyhow::Result<RenderReport> {
        // One instant for the whole frame, so the clock and the countdowns agree.
        let now = self.clock.now();
        let (weather, boards) = tokio::join!(
            async {
                let result = within(
                    self.limits.source_timeout,
                    self.weather_service.get_weather_for_location(location),
                )
                .await;
                self.last_weather.resolve(result, now, self.max_age.weather, "Weather")
            },
            futures_util::future::join_all(
                self.departure_boards.iter().map(|board| board.for_display(now, self.max_age.departures, self.limits.source_timeout)),
            ),
        );

        let date = DateInfo::new(now);
        let (departures, mut sources): (Vec<_>, Vec<_>) = boards.into_iter().unzip();
        let (glance_data, weather_state) = match weather {
            // Switched off, so there is nothing to report on.
            Fetched::Fresh(None) => (GlanceData::new(None, departures, date), None),
            Fetched::Fresh(weather) => (GlanceData::new(weather, departures, date), Some(SourceState::Fresh)),
            Fetched::Stale { value, age } => (
                GlanceData::new(value, departures, date).with_weather_age(age),
                Some(SourceState::Stale { age_seconds: age_seconds(age) }),
            ),
            Fetched::Unavailable { reason } => (
                GlanceData::new(None, departures, date).with_weather_unavailable(reason),
                Some(SourceState::Unavailable { reason: reason.to_owned() }),
            ),
        };
        sources.splice(0..0, weather_state.map(|state| SourceReport { name: "weather".to_owned(), state }));
        let profile = self.image_viewing_service.profile();
        let image_data = self
            .display_image_generator
            .generate(glance_data, &profile)
            .await?;
        self.image_repository.store(&image_data).await?;
        self.image_viewing_service.display(&image_data).await?;
        Ok(RenderReport { sources })
    }
}

#[cfg(test)]
mod tests {
    use chrono_tz::Europe::London;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;
    use crate::domain::models::departures::{DepartureService, DepartureStatus};
    use crate::domain::models::display::{DisplayProfile, Palette};
    use crate::domain::models::image::ImageData;
    use crate::adapters::clock::SystemClock;
    use crate::domain::models::weather::WeatherCondition;

    type Flag = Arc<AtomicBool>;

    struct Weather(Flag);
    impl WeatherService for Weather {
        async fn get_weather_for_location(&self, _: Location) -> Result<Option<WeatherInformation>, SourceError> {
            if self.0.load(Ordering::SeqCst) {
                return Err(SourceError::Unauthorized { status: 401 });
            }
            Ok(Some(WeatherInformation::new(12, 8, 15, WeatherCondition::Clouds)))
        }
    }

    struct Trains(Flag, Flag);
    impl DeparturesService for Trains {
        async fn get_departures(&self, _: u8, now: DateTime<Tz>) -> Result<Departures, SourceError> {
            if self.1.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if self.0.load(Ordering::SeqCst) {
                return Err(SourceError::Upstream { status: 503 });
            }
            let at = |minutes: i64| (now + Duration::minutes(minutes)).format("%H:%M").to_string();
            let service = |minutes: i64| {
                DepartureService::new(at(minutes), "Moorgate".into(), DepartureStatus::OnTime, String::new(), format!("{minutes} min"))
            };
            Ok(Departures::new("Hornsey".into(), vec![service(10), service(25)]))
        }
    }

    /// Keeps what the dashboard was asked to draw.
    struct Capture(Arc<Mutex<Vec<serde_json::Value>>>, Flag);
    impl DisplayImageGenerator for Capture {
        async fn generate(&self, data: GlanceData, _: &DisplayProfile) -> anyhow::Result<ImageData> {
            if self.1.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            self.0.lock().unwrap().push(serde_json::to_value(&data)?);
            Ok(ImageData::new(vec![]))
        }
    }

    struct Panel;
    #[async_trait]
    impl ImageDisplayService for Panel {
        fn profile(&self) -> DisplayProfile {
            DisplayProfile { width: 1, height: 1, palette: Palette::Mono }
        }
        async fn display(&self, _: &ImageData) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct Store;
    #[async_trait]
    impl ImageRepository for Store {
        async fn store(&self, _: &ImageData) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct Rig {
        weather_down: Flag,
        trains_down: Flag,
        trains_hang: Flag,
        render_hang: Flag,
        frames: Arc<Mutex<Vec<serde_json::Value>>>,
        app: Application<Weather, Capture, Panel, Store, Trains>,
    }

    fn rig() -> Rig {
        let (weather_down, trains_down) = (Flag::default(), Flag::default());
        let (trains_hang, render_hang) = (Flag::default(), Flag::default());
        let frames = Arc::new(Mutex::new(Vec::new()));
        let app = Application::new(
            Weather(weather_down.clone()),
            Capture(frames.clone(), render_hang.clone()),
            Panel,
            Store,
            vec![DepartureBoard::new("NORTHBOUND".into(), 4, Trains(trains_down.clone(), trains_hang.clone()))],
            MaxAge::default(),
            RenderLimits { source_timeout: StdDuration::from_millis(100), deadline: StdDuration::from_millis(400) },
            Arc::new(SystemClock::new(London)),
        );
        Rig { weather_down, trains_down, trains_hang, render_hang, frames, app }
    }

    impl Rig {
        async fn run(&self) -> serde_json::Value {
            self.app.run(Location::new(0.0, 0.0)).await.expect("a failing source doesn't fail the render");
            self.frames.lock().unwrap().last().unwrap().clone()
        }
    }

    #[tokio::test]
    async fn the_report_says_how_each_source_did() {
        use crate::domain::models::render_report::SourceState;

        let fresh = rig();
        let rig = rig();
        let report = rig.app.run(Location::new(0.0, 0.0)).await.unwrap();
        assert!(!report.is_degraded());
        let names: Vec<_> = report.sources.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["weather", "NORTHBOUND"]);

        rig.trains_down.store(true, Ordering::SeqCst);
        rig.weather_down.store(true, Ordering::SeqCst);
        let report = rig.app.run(Location::new(0.0, 0.0)).await.unwrap();
        assert!(report.is_degraded());
        // The weather had worked, so it is stale; the board too.
        assert!(matches!(report.sources[0].state, SourceState::Stale { .. }));
        assert!(matches!(report.sources[1].state, SourceState::Stale { .. }));

        fresh.weather_down.store(true, Ordering::SeqCst);
        let report = fresh.app.run(Location::new(0.0, 0.0)).await.unwrap();
        assert_eq!(
            report.sources[0].state,
            SourceState::Unavailable { reason: "key rejected".to_owned() }
        );
    }

    #[tokio::test]
    async fn healthy_sources_give_an_unmarked_dashboard() {
        let frame = rig().run().await;
        assert_eq!(frame["departures"][0]["age"], "");
        assert_eq!(frame["departures"][0]["unavailable"], false);
        assert_eq!(frame["weather_age"], "");
        assert_eq!(frame["weather_unavailable"], false);
        assert_eq!(frame["departures"][0]["services"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_failed_board_is_shown_from_its_last_good_data_with_its_age() {
        let rig = rig();
        rig.run().await;
        rig.trains_down.store(true, Ordering::SeqCst);

        let frame = rig.run().await;
        let board = &frame["departures"][0];
        assert_eq!(board["unavailable"], false);
        assert_eq!(board["age"], "under 1 min");
        assert_eq!(board["station"], "Hornsey");
        assert_eq!(board["services"].as_array().unwrap().len(), 2);
        // Weather is fine, and not marked.
        assert_eq!(frame["weather_age"], "");
        assert!(frame["weather_information"].is_object());
    }

    #[tokio::test]
    async fn a_board_that_has_never_worked_is_unavailable_but_the_rest_still_draws() {
        let rig = rig();
        rig.trains_down.store(true, Ordering::SeqCst);

        let frame = rig.run().await;
        let board = &frame["departures"][0];
        assert_eq!(board["unavailable"], true);
        assert_eq!(board["reason"], "service error");
        assert_eq!(board["name"], "NORTHBOUND");
        assert!(board["services"].as_array().unwrap().is_empty());
        assert!(frame["weather_information"].is_object());
        // The heading and its one line, so the template can size the text.
        assert_eq!(frame["departure_lines"], 2);
    }

    #[tokio::test]
    async fn weather_that_fails_after_working_is_shown_with_its_age() {
        let rig = rig();
        rig.run().await;
        rig.weather_down.store(true, Ordering::SeqCst);

        let frame = rig.run().await;
        assert_eq!(frame["weather_age"], "under 1 min");
        assert_eq!(frame["weather_unavailable"], false);
        assert!(frame["weather_information"].is_object());
        assert_eq!(frame["departures"][0]["age"], "");
    }

    #[tokio::test]
    async fn weather_that_never_worked_is_marked_unavailable() {
        let rig = rig();
        rig.weather_down.store(true, Ordering::SeqCst);

        let frame = rig.run().await;
        assert_eq!(frame["weather_unavailable"], true);
        assert!(frame["weather_information"].is_null());
        // A rejected key is said so, rather than a vague "unavailable".
        assert_eq!(frame["weather_reason"], "key rejected");
    }

    #[tokio::test]
    async fn data_past_its_age_limit_is_unavailable() {
        let rig = {
            let mut rig = rig();
            rig.app.max_age = MaxAge { departures: Duration::seconds(-1), weather: Duration::seconds(-1) };
            rig
        };
        rig.run().await;
        rig.trains_down.store(true, Ordering::SeqCst);
        rig.weather_down.store(true, Ordering::SeqCst);

        let frame = rig.run().await;
        assert_eq!(frame["departures"][0]["unavailable"], true);
        assert_eq!(frame["weather_unavailable"], true);
    }

    #[tokio::test]
    async fn a_source_that_hangs_is_treated_as_failed_and_the_render_goes_on() {
        let rig = rig();
        rig.trains_hang.store(true, Ordering::SeqCst);

        let started = std::time::Instant::now();
        let frame = rig.run().await;
        assert!(started.elapsed() < StdDuration::from_millis(350), "{:?}", started.elapsed());
        assert_eq!(frame["departures"][0]["unavailable"], true);
        assert_eq!(frame["departures"][0]["reason"], "timed out");
        assert!(frame["weather_information"].is_object());
    }

    #[tokio::test]
    async fn a_hung_source_shows_its_earlier_data_like_any_other_failure() {
        let rig = rig();
        rig.run().await;
        rig.trains_hang.store(true, Ordering::SeqCst);

        let frame = rig.run().await;
        assert_eq!(frame["departures"][0]["age"], "under 1 min");
        assert_eq!(frame["departures"][0]["unavailable"], false);
    }

    #[tokio::test]
    async fn a_run_that_overshoots_the_deadline_is_abandoned_with_an_error() {
        let rig = rig();
        rig.render_hang.store(true, Ordering::SeqCst);

        let started = std::time::Instant::now();
        let error = rig.app.run(Location::new(0.0, 0.0)).await.unwrap_err();
        assert!(started.elapsed() < StdDuration::from_secs(2), "{:?}", started.elapsed());
        assert!(format!("{error}").contains("The render didn't answer within"), "{error}");

        // The next run is unaffected.
        rig.render_hang.store(false, Ordering::SeqCst);
        rig.run().await;
    }

    #[derive(Default)]
    struct Events(Mutex<Vec<&'static str>>);

    impl RenderObserver for Events {
        fn render_started(&self) {
            self.0.lock().unwrap().push("started");
        }
        fn render_succeeded(&self, _at: DateTime<Tz>, _report: &RenderReport) {
            self.0.lock().unwrap().push("succeeded");
        }
        fn render_failed(&self, _at: DateTime<Tz>, _error: &anyhow::Error) {
            self.0.lock().unwrap().push("failed");
        }
    }

    #[tokio::test]
    async fn observers_hear_how_each_run_starts_and_ends() {
        let events = Arc::new(Events::default());
        let rig = rig();
        let app = rig.app.with_observer(events.clone());

        app.run(Location::new(0.0, 0.0)).await.unwrap();
        assert_eq!(*events.0.lock().unwrap(), ["started", "succeeded"]);

        // A run abandoned at its deadline is a failure too.
        rig.render_hang.store(true, Ordering::SeqCst);
        app.run(Location::new(0.0, 0.0)).await.unwrap_err();
        assert_eq!(*events.0.lock().unwrap(), ["started", "succeeded", "started", "failed"]);
    }

    #[tokio::test]
    async fn the_frame_and_the_outcome_are_stamped_from_the_clock() {
        use chrono::TimeZone;

        use crate::adapters::clock::FixedClock;

        #[derive(Default)]
        struct Stamps(Mutex<Vec<DateTime<Tz>>>);
        impl RenderObserver for Stamps {
            fn render_started(&self) {}
            fn render_succeeded(&self, at: DateTime<Tz>, _report: &RenderReport) {
                self.0.lock().unwrap().push(at);
            }
            fn render_failed(&self, _at: DateTime<Tz>, _error: &anyhow::Error) {}
        }

        let rig = rig();
        let at = London.with_ymd_and_hms(2026, 6, 15, 9, 41, 0).unwrap();
        let stamps = Arc::new(Stamps::default());
        let app = Application::new(
            Weather(rig.weather_down.clone()),
            Capture(rig.frames.clone(), rig.render_hang.clone()),
            Panel,
            Store,
            vec![DepartureBoard::new("NORTHBOUND".into(), 4, Trains(rig.trains_down.clone(), rig.trains_hang.clone()))],
            MaxAge::default(),
            RenderLimits { source_timeout: StdDuration::from_millis(100), deadline: StdDuration::from_millis(400) },
            FixedClock::at(at),
        )
        .with_observer(stamps.clone());

        app.run(Location::new(0.0, 0.0)).await.unwrap();

        let frame = rig.frames.lock().unwrap().last().unwrap().to_string();
        assert!(frame.contains("09:41") && frame.contains("Mon"), "{frame}");
        assert_eq!(*stamps.0.lock().unwrap(), [at]);
    }

    /// A renderer that panics while the flag is set, and otherwise answers.
    struct Unreliable(Flag);
    impl DisplayImageGenerator for Unreliable {
        async fn generate(&self, _: GlanceData, _: &DisplayProfile) -> anyhow::Result<ImageData> {
            if self.0.load(Ordering::SeqCst) {
                panic!("Chrome sent something unreadable");
            }
            Ok(ImageData::new(vec![]))
        }
    }

    #[tokio::test]
    async fn a_panic_is_a_failed_run_that_the_observers_hear_about_and_the_next_run_recovers() {
        let events = Arc::new(Events::default());
        let broken = Flag::default();
        let app = Application::new(
            Weather(Flag::default()),
            Unreliable(broken.clone()),
            Panel,
            Store,
            vec![DepartureBoard::new("NORTHBOUND".into(), 4, Trains(Flag::default(), Flag::default()))],
            MaxAge::default(),
            RenderLimits { source_timeout: StdDuration::from_millis(100), deadline: StdDuration::from_millis(400) },
            Arc::new(SystemClock::new(London)),
        )
        .with_observer(events.clone());

        broken.store(true, Ordering::SeqCst);
        let error = app.run(Location::new(0.0, 0.0)).await.unwrap_err();
        assert!(format!("{error}").contains("The render panicked: Chrome sent something unreadable"), "{error}");
        assert_eq!(*events.0.lock().unwrap(), ["started", "failed"]);

        // The data sources' memory and the observers are intact: the next run works.
        broken.store(false, Ordering::SeqCst);
        app.run(Location::new(0.0, 0.0)).await.unwrap();
        assert_eq!(*events.0.lock().unwrap(), ["started", "failed", "started", "succeeded"]);
    }
}
