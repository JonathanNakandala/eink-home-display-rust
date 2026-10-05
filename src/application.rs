use chrono::{DateTime, Duration, Local};

use crate::domain::models::departures::Departures;
use crate::domain::models::freshness::{Fetched, LastGood};
use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;
use crate::domain::models::{DateInfo, DepartureBoardData, GlanceData};
use crate::domain::services::departures_service::DeparturesService;
use crate::domain::services::display_image_generator::DisplayImageGenerator;
use crate::domain::services::image_repository::ImageRepository;
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
    pub async fn fetch(&self, rows: u8, now: DateTime<Local>) -> anyhow::Result<Departures> {
        self.service.get_departures(rows, now).await
    }

    /// The board as it should be shown: fresh if the source answered, otherwise what it last
    /// said (brought up to date, and labelled with its age) if that is recent enough.
    async fn for_display(&self, now: DateTime<Local>, max_age: Duration) -> DepartureBoardData {
        let result = self.fetch(self.rows, now).await;
        let source = format!("Departures for {}", self.name);
        match self.last_good.resolve(result, now, max_age, &source) {
            Fetched::Fresh(departures) => {
                DepartureBoardData::new(self.name.clone(), departures.station, departures.services)
            }
            Fetched::Stale { value, age } => {
                let departures = value.as_of(now);
                DepartureBoardData::from_earlier(self.name.clone(), departures.station, departures.services, age)
            }
            Fetched::Unavailable => DepartureBoardData::unavailable(self.name.clone()),
        }
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
    #[new(default)]
    last_weather: LastGood<Option<WeatherInformation>>,
}

impl<WS, DIG, IDS, IR, DS> Application<WS, DIG, IDS, IR, DS>
where
    WS: WeatherService,
    DIG: DisplayImageGenerator,
    IDS: ImageDisplayService,
    IR: ImageRepository,
    DS: DeparturesService,
{
    /// Renders and shows the dashboard. A source that fails doesn't stop it: that part is shown
    /// from its last good data if recent enough (labelled with the age), or as unavailable.
    pub async fn run(&self, location: Location) -> anyhow::Result<()> {
        // One instant for the whole frame, so the clock and the countdowns agree.
        let now = chrono::Local::now();
        let (weather, departures) = tokio::join!(
            async {
                let result = self.weather_service.get_weather_for_location(location).await;
                self.last_weather.resolve(result, now, self.max_age.weather, "Weather")
            },
            futures_util::future::join_all(
                self.departure_boards.iter().map(|board| board.for_display(now, self.max_age.departures)),
            ),
        );

        let date = DateInfo::new(now);
        let glance_data = match weather {
            Fetched::Fresh(weather) => GlanceData::new(weather, departures, date),
            Fetched::Stale { value, age } => GlanceData::new(value, departures, date).with_weather_age(age),
            Fetched::Unavailable => GlanceData::new(None, departures, date).with_weather_unavailable(),
        };
        let profile = self.image_viewing_service.profile();
        let image_data = self
            .display_image_generator
            .generate(glance_data, &profile)
            .await?;
        self.image_repository.store(&image_data).await?;
        self.image_viewing_service.display(&image_data).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use anyhow::anyhow;
    use async_trait::async_trait;

    use super::*;
    use crate::domain::models::departures::{DepartureService, DepartureStatus};
    use crate::domain::models::display::{DisplayProfile, Palette};
    use crate::domain::models::image::ImageData;
    use crate::domain::models::weather::WeatherCondition;

    type Flag = Arc<AtomicBool>;

    struct Weather(Flag);
    impl WeatherService for Weather {
        async fn get_weather_for_location(&self, _: Location) -> anyhow::Result<Option<WeatherInformation>> {
            if self.0.load(Ordering::SeqCst) {
                return Err(anyhow!("weather is down"));
            }
            Ok(Some(WeatherInformation::new(12, 8, 15, WeatherCondition::Clouds)))
        }
    }

    struct Trains(Flag);
    impl DeparturesService for Trains {
        async fn get_departures(&self, _: u8, now: DateTime<Local>) -> anyhow::Result<Departures> {
            if self.0.load(Ordering::SeqCst) {
                return Err(anyhow!("trains are down"));
            }
            let at = |minutes: i64| (now + Duration::minutes(minutes)).format("%H:%M").to_string();
            let service = |minutes: i64| {
                DepartureService::new(at(minutes), "Moorgate".into(), DepartureStatus::OnTime, String::new(), format!("{minutes} min"))
            };
            Ok(Departures::new("Hornsey".into(), vec![service(10), service(25)]))
        }
    }

    /// Keeps what the dashboard was asked to draw.
    struct Capture(Arc<Mutex<Vec<serde_json::Value>>>);
    impl DisplayImageGenerator for Capture {
        async fn generate(&self, data: GlanceData, _: &DisplayProfile) -> anyhow::Result<ImageData> {
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
        frames: Arc<Mutex<Vec<serde_json::Value>>>,
        app: Application<Weather, Capture, Panel, Store, Trains>,
    }

    fn rig() -> Rig {
        let (weather_down, trains_down) = (Flag::default(), Flag::default());
        let frames = Arc::new(Mutex::new(Vec::new()));
        let app = Application::new(
            Weather(weather_down.clone()),
            Capture(frames.clone()),
            Panel,
            Store,
            vec![DepartureBoard::new("NORTHBOUND".into(), 4, Trains(trains_down.clone()))],
            MaxAge::default(),
        );
        Rig { weather_down, trains_down, frames, app }
    }

    impl Rig {
        async fn run(&self) -> serde_json::Value {
            self.app.run(Location::new(0.0, 0.0)).await.expect("a failing source doesn't fail the render");
            self.frames.lock().unwrap().last().unwrap().clone()
        }
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
}
