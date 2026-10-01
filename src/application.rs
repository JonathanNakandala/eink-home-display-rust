use crate::domain::models::departures::StationPair;
use crate::domain::models::{GlanceData, NationalRailInformation};
use crate::domain::models::location::Location;
use crate::domain::services::departures_service::DeparturesService;
use crate::domain::services::display_image_generator::DisplayImageGenerator;
use crate::domain::services::image_repository::ImageRepository;
use crate::domain::services::ImageDisplayService;
use crate::domain::services::weather_service::WeatherService;

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
    departures_service: DS,
}

impl<WS, DIG, IDS, IR, DS> Application<WS, DIG, IDS, IR, DS>
where
    WS: WeatherService,
    DIG: DisplayImageGenerator,
    IDS: ImageDisplayService,
    IR: ImageRepository,
    DS: DeparturesService,
{
    pub async fn run(
        &self,
        location: Location,
        northbound: StationPair,
        southbound: StationPair,
    ) -> anyhow::Result<()> {
        let (weather_information, northbound_trains, southbound_trains) = tokio::try_join!(
            self.weather_service.get_weather_for_location(location),
            self.departures_service
                .get_departures(&northbound.from, &northbound.to, 4),
            self.departures_service
                .get_departures(&southbound.from, &southbound.to, 4),
        )?;

        let glance_data = GlanceData::new(
            weather_information,
            NationalRailInformation::new(northbound_trains, southbound_trains),
        );
        let image_data = self.display_image_generator.generate(glance_data).await?;
        self.image_repository.store(&image_data).await?;
        self.image_viewing_service.display(&image_data).await?;
        Ok(())
    }
}
