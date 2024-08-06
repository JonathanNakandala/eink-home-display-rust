use crate::domain::models::GlanceData;
use crate::domain::models::location::Location;
use crate::domain::services::display_image_generator::DisplayImageGenerator;
use crate::domain::services::image_repository::ImageRepository;
use crate::domain::services::ImageDisplayService;
use crate::domain::services::weather_service::WeatherService;

#[derive(derive_new::new)]
pub struct Application<WS, DIG, IDS, IR>
where
    WS: WeatherService,
    DIG: DisplayImageGenerator,
    IDS: ImageDisplayService,
    IR: ImageRepository,
{
    weather_service: WS,
    display_image_generator: DIG,
    image_viewing_service: IDS,
    image_repository: IR,
}

impl<WS, DIG, IDS, IR> Application<WS, DIG, IDS, IR>
where
    WS: WeatherService,
    DIG: DisplayImageGenerator,
    IDS: ImageDisplayService,
    IR: ImageRepository,
{
    pub async fn run(&self, location: Location) -> anyhow::Result<()> {
        let weather_information = self.weather_service.get_weather_for_location(location);
        let glance_data = GlanceData::new(weather_information.await?);
        let image_data = self.display_image_generator.generate(glance_data).await?;
        self.image_repository.store(&image_data).await?;
        self.image_viewing_service.display(&image_data).await?;
        Ok(())
    }
}
