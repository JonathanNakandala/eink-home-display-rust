use chrono::{DateTime, Local};

use crate::domain::models::departures::Departures;
use crate::domain::models::location::Location;
use crate::domain::models::{DateInfo, DepartureBoardData, GlanceData};
use crate::domain::services::departures_service::DeparturesService;
use crate::domain::services::display_image_generator::DisplayImageGenerator;
use crate::domain::services::image_repository::ImageRepository;
use crate::domain::services::weather_service::WeatherService;
use crate::domain::services::ImageDisplayService;

/// A titled list of departures from one configured source.
#[derive(derive_new::new)]
pub struct DepartureBoard<DS: DeparturesService> {
    name: String,
    rows: u8,
    service: DS,
}

impl<DS: DeparturesService> DepartureBoard<DS> {
    pub async fn fetch(&self, rows: u8, now: DateTime<Local>) -> anyhow::Result<Departures> {
        self.service.get_departures(rows, now).await
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
}

impl<WS, DIG, IDS, IR, DS> Application<WS, DIG, IDS, IR, DS>
where
    WS: WeatherService,
    DIG: DisplayImageGenerator,
    IDS: ImageDisplayService,
    IR: ImageRepository,
    DS: DeparturesService,
{
    pub async fn run(&self, location: Location) -> anyhow::Result<()> {
        // One instant for the whole frame, so the clock and the countdowns agree.
        let now = chrono::Local::now();
        let boards = futures_util::future::try_join_all(
            self.departure_boards.iter().map(|board| async move {
                let departures = board.fetch(board.rows, now).await?;
                anyhow::Ok(DepartureBoardData::new(
                    board.name.clone(),
                    departures.station,
                    departures.services,
                ))
            }),
        );
        let (weather_information, departures) = tokio::try_join!(
            self.weather_service.get_weather_for_location(location),
            boards,
        )?;

        let glance_data = GlanceData::new(
            weather_information,
            departures,
            DateInfo::new(now),
        );
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
