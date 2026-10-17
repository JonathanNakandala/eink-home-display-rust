pub mod api_schema;
pub mod devices;
pub mod enrollment;
pub mod handshakes;
pub mod launch;
pub mod plan;
pub mod refresh;
pub mod renewal_watch;
pub mod status;
pub mod viewers;

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use anyhow::anyhow;
use chrono::{DateTime, Duration};
use chrono_tz::Tz;
use futures_util::FutureExt;

use std::future::Future;

use crate::domain::models::departures::Departures;
use crate::domain::models::display::DisplayProfile;
use crate::domain::models::freshness::{Fetched, LastGood};
use crate::domain::models::layout::{Layout, Layouts};
use crate::domain::models::location::Location;
use crate::domain::models::render_report::{
    LayoutReport, LayoutState, RenderReport, SourceReport, SourceState,
};
use crate::domain::models::source_error::SourceError;
use crate::domain::models::weather::WeatherInformation;
use crate::domain::models::{DateInfo, DepartureBoardData, GlanceData};
use crate::domain::services::ImageDisplayService;
use crate::domain::services::clock::Clock;
use crate::domain::services::departures_service::DeparturesService;
use crate::domain::services::display_image_generator::DisplayImageGenerator;
use crate::domain::services::image_repository::ImageRepository;
use crate::domain::services::render_observer::RenderObserver;
use crate::domain::services::weather_service::WeatherService;

/// How old data from an earlier fetch may be when its source fails, before it is dropped
/// from the display as unavailable.
#[derive(Debug, Clone, Copy)]
pub struct MaxAge {
    pub departures: Duration,
    pub weather: Duration,
}

impl Default for MaxAge {
    fn default() -> Self {
        Self {
            departures: Duration::minutes(15),
            weather: Duration::minutes(180),
        }
    }
}

/// How long a run, and each part of it, may take.
#[derive(Debug, Clone, Copy)]
pub struct RenderLimits {
    /// For one source (the weather, or one board) to answer. A source that is too slow counts as
    /// failed, so it is shown from its last good data or as unavailable instead of holding up the rest.
    pub source_timeout: StdDuration,
    /// For fetching the sources and drawing the default layout, which is what makes a run. Whatever is
    /// still going then is abandoned, so a stuck render can't stop the next one or a waiting button press.
    pub deadline: StdDuration,
    /// For each layout after the default to be drawn and published. Each has its own allowance, so that more
    /// layouts don't squeeze the run's, and one that is too slow is reported and left with its last image
    /// while the rest carry on.
    pub layout_deadline: StdDuration,
}

impl Default for RenderLimits {
    fn default() -> Self {
        Self {
            source_timeout: StdDuration::from_secs(40),
            deadline: StdDuration::from_secs(120),
            layout_deadline: StdDuration::from_secs(60),
        }
    }
}

/// How a render treats a source that is slow or down: how old its last good result may be, and how long to wait
/// for it. The two are set together (`[stale_data]` and `[limits]` in the configuration) and used together.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderPolicy {
    pub max_age: MaxAge,
    pub limits: RenderLimits,
}

/// A source's answer, or `SourceError::Timeout` if it takes longer than `limit`.
async fn within<T>(
    limit: StdDuration,
    work: impl Future<Output = Result<T, SourceError>>,
) -> Result<T, SourceError> {
    tokio::time::timeout(limit, work)
        .await
        .unwrap_or(Err(SourceError::Timeout))
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
                    DepartureBoardData::from_earlier(
                        self.name.clone(),
                        departures.station,
                        departures.services,
                        age,
                    ),
                    SourceState::Stale {
                        age_seconds: age_seconds(age),
                    },
                )
            }
            Fetched::Unavailable { reason } => (
                DepartureBoardData::unavailable(self.name.clone(), reason),
                SourceState::Unavailable {
                    reason: reason.to_owned(),
                },
            ),
        };
        (
            board,
            SourceReport {
                name: self.name.clone(),
                state,
            },
        )
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
    policy: RenderPolicy,
    clock: Arc<dyn Clock>,
    /// The layouts to draw; see `with_layouts`.
    #[new(default)]
    layouts: Layouts,
    /// Outputs besides the display: each is drawn for as well, at its own size; see `with_output`.
    #[new(default)]
    extra_outputs: Vec<Arc<dyn ImageDisplayService>>,
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

    /// Has `output` drawn for on each run as well as the display, from the same data, at its own size: a web page, say, beside
    /// an e-ink panel. It is not the display, so one layout that cannot be drawn for it is reported and the run goes on, as
    /// with any layout besides the default.
    pub fn with_output(mut self, output: Arc<dyn ImageDisplayService>) -> Self {
        self.extra_outputs.push(output);
        self
    }

    /// Has `layouts` drawn on each run (the first is the default) in place of the dashboard alone.
    pub fn with_layouts(mut self, layouts: Layouts) -> Self {
        self.layouts = layouts;
        self
    }

    /// Renders and shows the dashboard. A source that fails or is too slow doesn't stop it: that part
    /// is shown from its last good data if recent enough (labelled with the age), or as unavailable.
    /// Fetching and drawing the default layout is abandoned, with an error, if it takes longer than `limits.deadline`;
    /// each other layout has `limits.layout_deadline` and, if it overruns, is reported and left as it was.
    /// On success, says how healthy each source was. The observers hear of the outcome either way.
    ///
    /// A panic in a source, the renderer or the display is caught and reported as a failed run, like
    /// any other: the service also serves the image and its health, and one bad response must not end it.
    pub async fn run(&self, location: Location) -> anyhow::Result<RenderReport> {
        for observer in &self.observers {
            observer.render_started();
        }
        let result = match AssertUnwindSafe(self.render(location)).catch_unwind().await {
            Ok(rendered) => rendered,
            Err(panic) => Err(anyhow!(
                "The render panicked: {}",
                panic_message(panic.as_ref())
            )),
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

    /// Fetches the sources, then draws the default layout, within `limits.deadline` together.
    async fn render(&self, location: Location) -> anyhow::Result<RenderReport> {
        // One instant for the whole frame, so the clock and the countdowns agree.
        let now = self.clock.now();
        let profile = self.image_viewing_service.profile();
        // A panel that is driven directly shows one picture, so the others would be drawn for nothing.
        // ...and the one it shows is the one for this time of day, when there is a schedule.
        let layouts = if self.image_viewing_service.wants_every_layout() {
            self.layouts.all()
        } else {
            std::slice::from_ref(self.layouts.current(now))
        };
        let (first, others) = layouts
            .split_first()
            .expect("there is always at least one layout");

        let deadline = self.policy.limits.deadline;
        let essentials = async {
            let (glance_data, sources) = self.gather(location, now).await;
            // Without the default there is no picture worth the name, and whatever went wrong would likely go
            // wrong again for the rest, so this failing fails the run.
            self.show(
                &self.image_viewing_service,
                true,
                first,
                &glance_data,
                &profile,
            )
            .await?;
            anyhow::Ok((glance_data, sources))
        };
        let (glance_data, sources) = tokio::time::timeout(deadline, essentials)
            .await
            .map_err(|_| anyhow!("The render didn't answer within {deadline:?}"))??;

        let name = self.image_viewing_service.name();
        let mut drawn = vec![LayoutReport {
            output: name.to_owned(),
            name: first.name.to_string(),
            state: LayoutState::Drawn,
        }];
        for layout in others {
            drawn.push(LayoutReport {
                output: name.to_owned(),
                name: layout.name.to_string(),
                state: self
                    .draw(
                        &self.image_viewing_service,
                        true,
                        layout,
                        &glance_data,
                        &profile,
                    )
                    .await,
            });
        }
        // The display's pictures are done: whatever is waiting for them (a button press) need not wait for the outputs besides.
        for observer in &self.observers {
            observer.display_updated();
        }
        for output in &self.extra_outputs {
            let (profile, layouts) = (output.profile(), self.layouts_for(output.as_ref(), now));
            for layout in layouts {
                drawn.push(LayoutReport {
                    output: output.name().to_owned(),
                    name: layout.name.to_string(),
                    state: self
                        .draw(output.as_ref(), false, layout, &glance_data, &profile)
                        .await,
                });
            }
        }
        Ok(RenderReport {
            sources,
            layouts: drawn,
        })
    }

    /// The layouts `output` is given: all of them if it serves them to be chosen from, else the one for this time of day.
    fn layouts_for(&self, output: &dyn ImageDisplayService, now: DateTime<Tz>) -> &[Layout] {
        if output.wants_every_layout() {
            self.layouts.all()
        } else {
            std::slice::from_ref(self.layouts.current(now))
        }
    }

    /// Draws a layout for an output that can do without it: one that fails or is slow is reported and left with its last
    /// image, which shows as degraded, and must not take the others with it.
    async fn draw(
        &self,
        output: &dyn ImageDisplayService,
        keep: bool,
        layout: &Layout,
        glance_data: &GlanceData,
        profile: &DisplayProfile,
    ) -> LayoutState {
        let outcome = tokio::time::timeout(
            self.policy.limits.layout_deadline,
            self.show(output, keep, layout, glance_data, profile),
        )
        .await;
        match outcome {
            Ok(Ok(())) => LayoutState::Drawn,
            Ok(Err(error)) => {
                log::warn!(
                    "Failed to draw the {} layout for {}: {error:#}",
                    layout.name,
                    output.name()
                );
                LayoutState::Failed {
                    reason: "could not be drawn".to_owned(),
                }
            }
            Err(_) => {
                log::warn!(
                    "The {} layout for {} took longer than {:?}; its last image stays",
                    layout.name,
                    output.name(),
                    self.policy.limits.layout_deadline
                );
                LayoutState::Failed {
                    reason: "timed out".to_owned(),
                }
            }
        }
    }

    /// What the dashboard shows, from the sources, and how each did. A source that fails or is too slow doesn't stop it.
    async fn gather(
        &self,
        location: Location,
        now: DateTime<Tz>,
    ) -> (GlanceData, Vec<SourceReport>) {
        let (weather, boards) = tokio::join!(
            async {
                let result = within(
                    self.policy.limits.source_timeout,
                    self.weather_service.get_weather_for_location(location),
                )
                .await;
                self.last_weather
                    .resolve(result, now, self.policy.max_age.weather, "Weather")
            },
            futures_util::future::join_all(self.departure_boards.iter().map(|board| {
                board.for_display(
                    now,
                    self.policy.max_age.departures,
                    self.policy.limits.source_timeout,
                )
            }),),
        );

        let date = DateInfo::new(now);
        let (departures, mut sources): (Vec<_>, Vec<_>) = boards.into_iter().unzip();
        let (glance_data, weather_state) = match weather {
            // Switched off, so there is nothing to report on.
            Fetched::Fresh(None) => (GlanceData::new(None, departures, date), None),
            Fetched::Fresh(weather) => (
                GlanceData::new(weather, departures, date),
                Some(SourceState::Fresh),
            ),
            Fetched::Stale { value, age } => (
                GlanceData::new(value, departures, date).with_weather_age(age),
                Some(SourceState::Stale {
                    age_seconds: age_seconds(age),
                }),
            ),
            Fetched::Unavailable { reason } => (
                GlanceData::new(None, departures, date).with_weather_unavailable(reason),
                Some(SourceState::Unavailable {
                    reason: reason.to_owned(),
                }),
            ),
        };
        sources.splice(
            0..0,
            weather_state.map(|state| SourceReport {
                name: "weather".to_owned(),
                state,
            }),
        );
        (glance_data, sources)
    }

    /// Draws one layout for `output`, keeps the picture if `keep` and hands it to the output.
    async fn show(
        &self,
        output: &dyn ImageDisplayService,
        keep: bool,
        layout: &Layout,
        glance_data: &GlanceData,
        profile: &DisplayProfile,
    ) -> anyhow::Result<()> {
        let image_data = self
            .display_image_generator
            .generate(glance_data.clone(), profile, &layout.name)
            .await?;
        if keep {
            self.image_repository
                .store(&layout.name, &image_data)
                .await?;
        }
        output.display(&layout.name, &image_data).await
    }
}

#[cfg(test)]
mod tests {
    use chrono_tz::Europe::London;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;
    use crate::adapters::clock::SystemClock;
    use crate::domain::models::departures::{DepartureService, DepartureStatus};
    use crate::domain::models::display::Palette;
    use crate::domain::models::image::ImageData;
    use crate::domain::models::layout::LayoutName;
    use crate::domain::models::weather::WeatherCondition;

    type Flag = Arc<AtomicBool>;

    struct Weather(Flag);
    impl WeatherService for Weather {
        async fn get_weather_for_location(
            &self,
            _: Location,
        ) -> Result<Option<WeatherInformation>, SourceError> {
            if self.0.load(Ordering::SeqCst) {
                return Err(SourceError::Unauthorized { status: 401 });
            }
            Ok(Some(WeatherInformation::new(
                12,
                8,
                15,
                WeatherCondition::Clouds,
            )))
        }
    }

    struct Trains(Flag, Flag);
    impl DeparturesService for Trains {
        async fn get_departures(
            &self,
            _: u8,
            now: DateTime<Tz>,
        ) -> Result<Departures, SourceError> {
            if self.1.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if self.0.load(Ordering::SeqCst) {
                return Err(SourceError::Upstream { status: 503 });
            }
            let at = |minutes: i64| {
                (now + Duration::minutes(minutes))
                    .format("%H:%M")
                    .to_string()
            };
            let service = |minutes: i64| {
                DepartureService::new(
                    at(minutes),
                    "Moorgate".into(),
                    DepartureStatus::OnTime,
                    String::new(),
                    format!("{minutes} min"),
                )
            };
            Ok(Departures::new(
                "Hornsey".into(),
                vec![service(10), service(25)],
            ))
        }
    }

    /// Keeps what the dashboard was asked to draw.
    struct Capture(Arc<Mutex<Vec<serde_json::Value>>>, Flag);
    impl DisplayImageGenerator for Capture {
        async fn generate(
            &self,
            data: GlanceData,
            _: &DisplayProfile,
            _: &LayoutName,
        ) -> anyhow::Result<ImageData> {
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
        fn name(&self) -> &'static str {
            "panel"
        }
        fn profile(&self) -> DisplayProfile {
            DisplayProfile {
                width: 1,
                height: 1,
                palette: Palette::Mono,
            }
        }
        async fn display(&self, _: &LayoutName, _: &ImageData) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct Store;
    #[async_trait]
    impl ImageRepository for Store {
        async fn store(&self, _: &LayoutName, _: &ImageData) -> anyhow::Result<()> {
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
            vec![DepartureBoard::new(
                "NORTHBOUND".into(),
                4,
                Trains(trains_down.clone(), trains_hang.clone()),
            )],
            RenderPolicy {
                limits: RenderLimits {
                    source_timeout: StdDuration::from_millis(100),
                    deadline: StdDuration::from_millis(400),
                    layout_deadline: StdDuration::from_millis(100),
                },
                ..RenderPolicy::default()
            },
            Arc::new(SystemClock::new(London)),
        );
        Rig {
            weather_down,
            trains_down,
            trains_hang,
            render_hang,
            frames,
            app,
        }
    }

    impl Rig {
        async fn run(&self) -> serde_json::Value {
            self.app
                .run(Location::new(0.0, 0.0))
                .await
                .expect("a failing source doesn't fail the render");
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
            SourceState::Unavailable {
                reason: "key rejected".to_owned()
            }
        );
    }

    #[tokio::test]
    async fn healthy_sources_give_an_unmarked_dashboard() {
        let frame = rig().run().await;
        assert_eq!(frame["departures"][0]["age"], "");
        assert_eq!(frame["departures"][0]["unavailable"], false);
        assert_eq!(frame["weather_age"], "");
        assert_eq!(frame["weather_unavailable"], false);
        assert_eq!(
            frame["departures"][0]["services"].as_array().unwrap().len(),
            2
        );
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
            rig.app.policy.max_age = MaxAge {
                departures: Duration::seconds(-1),
                weather: Duration::seconds(-1),
            };
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
        assert!(
            started.elapsed() < StdDuration::from_millis(350),
            "{:?}",
            started.elapsed()
        );
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
        assert!(
            started.elapsed() < StdDuration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(
            format!("{error}").contains("The render didn't answer within"),
            "{error}"
        );

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
        assert_eq!(
            *events.0.lock().unwrap(),
            ["started", "succeeded", "started", "failed"]
        );
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
            vec![DepartureBoard::new(
                "NORTHBOUND".into(),
                4,
                Trains(rig.trains_down.clone(), rig.trains_hang.clone()),
            )],
            RenderPolicy {
                limits: RenderLimits {
                    source_timeout: StdDuration::from_millis(100),
                    deadline: StdDuration::from_millis(400),
                    layout_deadline: StdDuration::from_millis(100),
                },
                ..RenderPolicy::default()
            },
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
        async fn generate(
            &self,
            _: GlanceData,
            _: &DisplayProfile,
            _: &LayoutName,
        ) -> anyhow::Result<ImageData> {
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
            vec![DepartureBoard::new(
                "NORTHBOUND".into(),
                4,
                Trains(Flag::default(), Flag::default()),
            )],
            RenderPolicy {
                limits: RenderLimits {
                    source_timeout: StdDuration::from_millis(100),
                    deadline: StdDuration::from_millis(400),
                    layout_deadline: StdDuration::from_millis(100),
                },
                ..RenderPolicy::default()
            },
            Arc::new(SystemClock::new(London)),
        )
        .with_observer(events.clone());

        broken.store(true, Ordering::SeqCst);
        let error = app.run(Location::new(0.0, 0.0)).await.unwrap_err();
        assert!(
            format!("{error}").contains("The render panicked: Chrome sent something unreadable"),
            "{error}"
        );
        assert_eq!(*events.0.lock().unwrap(), ["started", "failed"]);

        // The data sources' memory and the observers are intact: the next run works.
        broken.store(false, Ordering::SeqCst);
        app.run(Location::new(0.0, 0.0)).await.unwrap();
        assert_eq!(
            *events.0.lock().unwrap(),
            ["started", "failed", "started", "succeeded"]
        );
    }

    /// Notes each layout it is asked for, in each of the three places, and fails on request.
    #[derive(Default)]
    struct Journal {
        generated: Mutex<Vec<String>>,
        stored: Mutex<Vec<String>>,
        shown: Mutex<Vec<String>>,
        failing: Mutex<Option<String>>,
        /// This layout never finishes.
        hanging: Mutex<Option<String>>,
        /// The layout and width of each picture asked for, which is how big it was drawn.
        sizes: Mutex<Vec<(String, u32)>>,
        every_layout: bool,
    }
    struct Recording(Arc<Journal>);
    impl DisplayImageGenerator for Recording {
        async fn generate(
            &self,
            _: GlanceData,
            profile: &DisplayProfile,
            layout: &LayoutName,
        ) -> anyhow::Result<ImageData> {
            self.0
                .sizes
                .lock()
                .unwrap()
                .push((layout.to_string(), profile.width));
            if self.0.failing.lock().unwrap().as_deref() == Some(layout.as_str()) {
                anyhow::bail!("Chrome fell over");
            }
            if self.0.hanging.lock().unwrap().as_deref() == Some(layout.as_str()) {
                std::future::pending::<()>().await;
            }
            self.0.generated.lock().unwrap().push(layout.to_string());
            Ok(ImageData::new(vec![]))
        }
    }
    #[async_trait]
    impl ImageRepository for Recording {
        async fn store(&self, layout: &LayoutName, _: &ImageData) -> anyhow::Result<()> {
            self.0.stored.lock().unwrap().push(layout.to_string());
            Ok(())
        }
    }
    #[async_trait]
    impl ImageDisplayService for Recording {
        fn name(&self) -> &'static str {
            "recording"
        }
        fn profile(&self) -> DisplayProfile {
            Panel.profile()
        }
        fn wants_every_layout(&self) -> bool {
            self.0.every_layout
        }
        async fn display(&self, layout: &LayoutName, _: &ImageData) -> anyhow::Result<()> {
            self.0.shown.lock().unwrap().push(layout.to_string());
            Ok(())
        }
    }

    fn two_layouts() -> Layouts {
        Layouts::new(vec![
            Layout::new("first", "The default"),
            Layout::new("second", "Another"),
        ])
        .unwrap()
    }

    fn recording_app(
        journal: &Arc<Journal>,
    ) -> Application<Weather, Recording, Recording, Recording, Trains> {
        recording_app_with(journal, Arc::new(SystemClock::new(London)))
    }

    fn recording_app_with(
        journal: &Arc<Journal>,
        clock: Arc<dyn Clock>,
    ) -> Application<Weather, Recording, Recording, Recording, Trains> {
        Application::new(
            Weather(Flag::default()),
            Recording(journal.clone()),
            Recording(journal.clone()),
            Recording(journal.clone()),
            vec![DepartureBoard::new(
                "NORTHBOUND".into(),
                4,
                Trains(Flag::default(), Flag::default()),
            )],
            RenderPolicy {
                limits: RenderLimits {
                    source_timeout: StdDuration::from_millis(100),
                    deadline: StdDuration::from_millis(400),
                    layout_deadline: StdDuration::from_millis(100),
                },
                ..RenderPolicy::default()
            },
            clock,
        )
        .with_layouts(two_layouts())
    }

    #[tokio::test]
    async fn every_layout_is_drawn_stored_and_shown_when_displays_fetch_them() {
        let journal = Arc::new(Journal {
            every_layout: true,
            ..Journal::default()
        });

        let report = recording_app(&journal)
            .run(Location::new(0.0, 0.0))
            .await
            .unwrap();

        for seen in [&journal.generated, &journal.stored, &journal.shown] {
            assert_eq!(*seen.lock().unwrap(), ["first", "second"]);
        }
        assert!(!report.is_degraded());
    }

    #[tokio::test]
    async fn a_panel_driven_directly_is_given_only_the_default_layout() {
        let journal = Arc::new(Journal::default());

        recording_app(&journal)
            .run(Location::new(0.0, 0.0))
            .await
            .unwrap();

        for seen in [&journal.generated, &journal.stored, &journal.shown] {
            assert_eq!(*seen.lock().unwrap(), ["first"]);
        }
    }

    #[tokio::test]
    async fn a_layout_that_fails_is_reported_and_the_others_still_appear() {
        let journal = Arc::new(Journal {
            every_layout: true,
            ..Journal::default()
        });
        *journal.failing.lock().unwrap() = Some("second".to_owned());

        let report = recording_app(&journal)
            .run(Location::new(0.0, 0.0))
            .await
            .expect("a layout besides the default doesn't fail the run");

        assert_eq!(*journal.shown.lock().unwrap(), ["first"]);
        assert!(report.is_degraded());
        // The sources were fine; it is the layout that is reported.
        assert!(report.sources.iter().all(|s| s.state == SourceState::Fresh));
        assert_eq!(
            report.layouts,
            [
                LayoutReport {
                    output: "recording".to_owned(),
                    name: "first".to_owned(),
                    state: LayoutState::Drawn
                },
                LayoutReport {
                    output: "recording".to_owned(),
                    name: "second".to_owned(),
                    state: LayoutState::Failed {
                        reason: "could not be drawn".to_owned()
                    }
                }
            ]
        );
    }

    #[tokio::test]
    async fn a_layout_that_hangs_is_given_up_on_alone_and_the_others_carry_on() {
        let journal = Arc::new(Journal {
            every_layout: true,
            ..Journal::default()
        });
        *journal.hanging.lock().unwrap() = Some("second".to_owned());
        let started = std::time::Instant::now();

        let report = recording_app(&journal)
            .run(Location::new(0.0, 0.0))
            .await
            .expect("one layout overrunning doesn't fail the run");

        // It took the layout's allowance (100 ms here), not the run's (400 ms) or forever.
        assert!(
            started.elapsed() < StdDuration::from_millis(390),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(*journal.shown.lock().unwrap(), ["first"]);
        assert_eq!(
            report.layouts[1].state,
            LayoutState::Failed {
                reason: "timed out".to_owned()
            }
        );
        assert!(report.is_degraded());
    }

    #[tokio::test]
    async fn each_layout_has_its_own_allowance_so_more_layouts_do_not_shorten_the_runs() {
        // Three layouts that each take 70 ms: more than the run's whole allowance (400 ms) would hold if it were
        // shared by seven, and more than one layout's (100 ms) in total, but each is within its own.
        struct Slow(Arc<Journal>);
        impl DisplayImageGenerator for Slow {
            async fn generate(
                &self,
                _: GlanceData,
                _: &DisplayProfile,
                layout: &LayoutName,
            ) -> anyhow::Result<ImageData> {
                tokio::time::sleep(StdDuration::from_millis(70)).await;
                self.0.generated.lock().unwrap().push(layout.to_string());
                Ok(ImageData::new(vec![]))
            }
        }
        let journal = Arc::new(Journal {
            every_layout: true,
            ..Journal::default()
        });
        let layouts = Layouts::new(
            ["a", "b", "c", "d", "e", "f"]
                .iter()
                .map(|name| Layout::new(name, "A layout"))
                .collect(),
        )
        .unwrap();
        let app = Application::<_, _, _, _, Trains>::new(
            Weather(Flag::default()),
            Slow(journal.clone()),
            Recording(journal.clone()),
            Recording(journal.clone()),
            vec![],
            RenderPolicy {
                limits: RenderLimits {
                    source_timeout: StdDuration::from_millis(100),
                    deadline: StdDuration::from_millis(400),
                    layout_deadline: StdDuration::from_millis(100),
                },
                ..RenderPolicy::default()
            },
            Arc::new(SystemClock::new(London)),
        )
        .with_layouts(layouts);

        let report = app.run(Location::new(0.0, 0.0)).await.unwrap();

        // Six layouts of 70 ms is 420 ms, past the run's 400 ms, and every one was drawn.
        assert_eq!(journal.generated.lock().unwrap().len(), 6);
        assert!(!report.is_degraded());
    }

    #[tokio::test]
    async fn the_default_layout_failing_fails_the_run() {
        let journal = Arc::new(Journal {
            every_layout: true,
            ..Journal::default()
        });
        *journal.failing.lock().unwrap() = Some("first".to_owned());

        let error = recording_app(&journal)
            .run(Location::new(0.0, 0.0))
            .await
            .unwrap_err();

        assert!(format!("{error}").contains("Chrome fell over"), "{error}");
        assert!(journal.shown.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_panel_driven_directly_is_given_the_layout_for_the_time_of_day() {
        use crate::domain::models::layout::{LayoutRule, LayoutSchedule};
        use chrono::{NaiveTime, TimeZone};

        let journal = Arc::new(Journal::default());
        let morning = LayoutRule::new(
            LayoutName::parse("second").unwrap(),
            vec![],
            NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
        )
        .unwrap();
        let layouts = two_layouts()
            .with_schedule(LayoutSchedule::new(vec![morning]))
            .unwrap();
        let app_at = |hour: u32| {
            recording_app_with(
                &journal,
                crate::adapters::clock::FixedClock::at(
                    London.with_ymd_and_hms(2026, 10, 12, hour, 0, 0).unwrap(),
                ),
            )
            .with_layouts(layouts.clone())
        };

        app_at(7).run(Location::new(0.0, 0.0)).await.unwrap();
        app_at(12).run(Location::new(0.0, 0.0)).await.unwrap();

        // At seven the schedule puts the second layout on the panel; at noon it is the default again.
        assert_eq!(*journal.shown.lock().unwrap(), ["second", "first"]);
    }

    // --- outputs besides the display ------------------------------------------------------------------------

    /// A second output, like a web page beside an e-ink panel: drawn for at its own size, for the layouts it asks for.
    struct Screen {
        journal: Arc<Journal>,
        every_layout: bool,
        shown: Mutex<Vec<String>>,
        fail: bool,
    }
    #[async_trait]
    impl ImageDisplayService for Screen {
        fn name(&self) -> &'static str {
            "web"
        }
        fn profile(&self) -> DisplayProfile {
            DisplayProfile {
                width: 1920,
                height: 1080,
                palette: Palette::Rgb,
            }
        }
        fn wants_every_layout(&self) -> bool {
            self.every_layout
        }
        async fn display(&self, layout: &LayoutName, _: &ImageData) -> anyhow::Result<()> {
            if self.fail {
                anyhow::bail!("the page's folder is full");
            }
            self.shown.lock().unwrap().push(layout.to_string());
            let _ = &self.journal;
            Ok(())
        }
    }

    fn screen(journal: &Arc<Journal>, every_layout: bool, fail: bool) -> Arc<Screen> {
        Arc::new(Screen {
            journal: journal.clone(),
            every_layout,
            shown: Mutex::new(Vec::new()),
            fail,
        })
    }

    #[tokio::test]
    async fn another_output_is_drawn_for_at_its_own_size_from_the_same_data_and_reported_as_its_own()
     {
        let journal = Arc::new(Journal {
            every_layout: true,
            ..Journal::default()
        });
        let web = screen(&journal, true, false);

        let report = recording_app(&journal)
            .with_output(web.clone())
            .run(Location::new(0.0, 0.0))
            .await
            .unwrap();

        // The display got its layouts at its size (1, from the fake panel), and the page its own at 1920.
        let sizes = journal.sizes.lock().unwrap().clone();
        assert_eq!(
            sizes,
            [
                ("first".to_owned(), 1),
                ("second".to_owned(), 1),
                ("first".to_owned(), 1920),
                ("second".to_owned(), 1920)
            ]
        );
        assert_eq!(*web.shown.lock().unwrap(), ["first", "second"]);
        // The picture the display keeps is the display's alone: the page's are not stored as screenshots too.
        assert_eq!(*journal.stored.lock().unwrap(), ["first", "second"]);
        let outputs: Vec<_> = report
            .layouts
            .iter()
            .map(|l| (l.output.as_str(), l.name.as_str()))
            .collect();
        assert_eq!(
            outputs,
            [
                ("recording", "first"),
                ("recording", "second"),
                ("web", "first"),
                ("web", "second")
            ]
        );
        assert!(!report.is_degraded());
    }

    #[tokio::test]
    async fn an_output_that_fails_is_reported_and_the_display_and_the_run_are_untouched() {
        let journal = Arc::new(Journal {
            every_layout: true,
            ..Journal::default()
        });
        let web = screen(&journal, true, true);

        let report = recording_app(&journal)
            .with_output(web)
            .run(Location::new(0.0, 0.0))
            .await
            .expect("an output besides the display doesn't fail the run");

        assert_eq!(*journal.shown.lock().unwrap(), ["first", "second"]);
        assert!(report.is_degraded());
        let failed: Vec<_> = report
            .layouts
            .iter()
            .filter(|l| l.state != LayoutState::Drawn)
            .map(|l| (l.output.as_str(), l.name.as_str()))
            .collect();
        assert_eq!(failed, [("web", "first"), ("web", "second")]);
    }

    #[tokio::test]
    async fn an_output_that_serves_one_layout_gets_the_one_for_the_time_of_day_and_a_slow_one_is_given_up_on()
     {
        use crate::domain::models::layout::{LayoutRule, LayoutSchedule};
        use chrono::{NaiveTime, TimeZone};

        let journal = Arc::new(Journal {
            every_layout: true,
            ..Journal::default()
        });
        let morning = LayoutRule::new(
            LayoutName::parse("second").unwrap(),
            vec![],
            NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
        )
        .unwrap();
        let single = screen(&journal, false, false);
        recording_app_with(
            &journal,
            crate::adapters::clock::FixedClock::at(
                London.with_ymd_and_hms(2026, 10, 12, 7, 0, 0).unwrap(),
            ),
        )
        .with_layouts(
            two_layouts()
                .with_schedule(LayoutSchedule::new(vec![morning]))
                .unwrap(),
        )
        .with_output(single.clone())
        .run(Location::new(0.0, 0.0))
        .await
        .unwrap();
        assert_eq!(*single.shown.lock().unwrap(), ["second"]);

        // Drawing is the generator's, and a layout that never finishes is given up on whichever output it was for.
        *journal.hanging.lock().unwrap() = Some("second".to_owned());
        let slow = screen(&journal, true, false);
        let started = std::time::Instant::now();
        let report = recording_app(&journal)
            .with_output(slow.clone())
            .run(Location::new(0.0, 0.0))
            .await
            .unwrap();
        assert!(
            started.elapsed() < StdDuration::from_millis(390),
            "{:?}",
            started.elapsed()
        );
        assert!(report.is_degraded());
    }

    /// Notes, in order, what the display's pictures, the observers and another output do.
    #[derive(Default)]
    struct Order(Mutex<Vec<&'static str>>);
    impl Order {
        fn note(&self, what: &'static str) {
            self.0.lock().unwrap().push(what);
        }
    }
    impl RenderObserver for Order {
        fn render_started(&self) {
            self.note("started");
        }
        fn display_updated(&self) {
            self.note("display updated");
        }
        fn render_succeeded(&self, _: DateTime<Tz>, _: &RenderReport) {
            self.note("succeeded");
        }
        fn render_failed(&self, _: DateTime<Tz>, _: &anyhow::Error) {
            self.note("failed");
        }
    }
    /// An output besides the display, which says when it is given a picture.
    struct Noting(Arc<Order>);
    #[async_trait]
    impl ImageDisplayService for Noting {
        fn name(&self) -> &'static str {
            "web"
        }
        fn profile(&self) -> DisplayProfile {
            Panel.profile()
        }
        fn wants_every_layout(&self) -> bool {
            true
        }
        async fn display(&self, _: &LayoutName, _: &ImageData) -> anyhow::Result<()> {
            self.0.note("page drawn");
            Ok(())
        }
    }

    #[tokio::test]
    async fn observers_are_told_the_displays_pictures_are_done_before_the_outputs_besides_it_are_drawn()
     {
        let journal = Arc::new(Journal {
            every_layout: true,
            ..Journal::default()
        });
        let order = Arc::new(Order::default());

        recording_app(&journal)
            .with_output(Arc::new(Noting(order.clone())))
            .with_observer(order.clone())
            .run(Location::new(0.0, 0.0))
            .await
            .unwrap();

        assert_eq!(
            *order.0.lock().unwrap(),
            [
                "started",
                "display updated",
                "page drawn",
                "page drawn",
                "succeeded"
            ]
        );
    }

    #[tokio::test]
    async fn a_run_that_fails_before_the_display_is_done_never_says_it_was() {
        let journal = Arc::new(Journal {
            every_layout: true,
            ..Journal::default()
        });
        *journal.failing.lock().unwrap() = Some("first".to_owned());
        let order = Arc::new(Order::default());

        let result = recording_app(&journal)
            .with_output(Arc::new(Noting(order.clone())))
            .with_observer(order.clone())
            .run(Location::new(0.0, 0.0))
            .await;

        assert!(result.is_err());
        assert_eq!(*order.0.lock().unwrap(), ["started", "failed"]);
    }
}
