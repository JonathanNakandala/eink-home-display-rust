use crate::domain::models::location::Location;
use crate::domain::models::GlanceData;
use crate::domain::services::display_image_generator::DisplayImageGenerator;
use crate::domain::services::weather_service::LocalWeatherService;
use crate::domain::services::DisplayOutput;
use std::boxed::Box;
use std::path::PathBuf;
#[derive(derive_new::new)]
pub struct Application<WS: LocalWeatherService, DIG: DisplayImageGenerator> {
    weather_service: WS,
    display_image_generator: DIG,
    output: Box<dyn DisplayOutput>,
}

impl<WS, DIG> Application<WS, DIG>
where
    WS: LocalWeatherService,
    DIG: DisplayImageGenerator,
{
    pub async fn run(&self, location: Location) -> anyhow::Result<PathBuf> {
        let weather_information = self.weather_service.get_weather_for_location(location);

        let glance_data = GlanceData::new(weather_information.await?);

        let output_path = self.display_image_generator.generate(glance_data).await?;
        self.output.generate(vec![1, 2, 3]).await?;
        Ok(output_path)
    }
}
