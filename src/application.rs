use crate::domain::models::location::Location;
use crate::domain::models::GlanceData;
use crate::domain::services::display_image_generator::DisplayImageGenerator;
use crate::domain::services::weather_service::WeatherService;
use std::path::PathBuf;

#[derive(derive_new::new)]
pub struct Application<WS: WeatherService, DIG: DisplayImageGenerator> {
    weather_service: WS,
    display_image_generator: DIG,
}

impl<WS, DIG> Application<WS, DIG>
where
    WS: WeatherService,
    DIG: DisplayImageGenerator,
{
    pub async fn run(&self) -> anyhow::Result<PathBuf> {
        let weather_information = self
            .weather_service
            .get_weather_for_location(Location::new(20.0, 10.0));

        let glance_data = GlanceData::new(weather_information.await);

        let output_path = self.display_image_generator.generate(glance_data).await?;

        Ok(output_path)
    }
}
