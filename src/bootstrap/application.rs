use std::sync::Arc;
use std::time::Duration;

use crate::adapters::departures::DeparturesServiceImpl;
use crate::adapters::display_image_generator::chrome_render::{ChromeRenderDisplayImageGenerator, ChromeSource};
use crate::adapters::image_repository::file_store::FileStoreImageRepository;
use crate::adapters::weather::WeatherServiceImpl;
use crate::application::Application;
use crate::config::application::ApplicationConfig;
use crate::config::cache::CachePaths;
use crate::domain::services::display_image_generator::DisplayImageGenerator;
use crate::domain::services::image_repository::ImageRepository;
use crate::domain::services::published_images::PublishedImages;
use crate::domain::services::weather_service::WeatherService;
use crate::domain::services::departures_service::DeparturesService;
use crate::domain::services::ImageDisplayService;

use super::{setup_departure_boards, setup_display, setup_weather_service};

/// The application with the data sources the configuration asks for and the three ports that
/// differ between the programs that use it: how the image is drawn, shown and stored.
pub fn assemble<DIG, IDS, IR>(
    config: &ApplicationConfig,
    generator: DIG,
    display: IDS,
    repository: IR,
) -> anyhow::Result<Application<WeatherServiceImpl, DIG, IDS, IR, DeparturesServiceImpl>>
where
    DIG: DisplayImageGenerator,
    IDS: ImageDisplayService,
    IR: ImageRepository,
{
    Ok(Application::new(
        setup_weather_service(&config.weather)?,
        generator,
        display,
        repository,
        setup_departure_boards(&config.departures, &config.providers)?,
        (&config.stale_data).into(),
        (&config.limits).into(),
    ))
}

/// The service as it runs: rendered by Chrome, shown on the configured display, and saved to the
/// configured folder. `images` is where displays that fetch their image have it published.
pub fn from_config(
    config: &ApplicationConfig,
    cache: &CachePaths,
    chrome_idle_timeout: Duration,
    chrome_source: ChromeSource,
    images: Arc<dyn PublishedImages>,
) -> anyhow::Result<
    Application<
        impl WeatherService,
        impl DisplayImageGenerator,
        impl ImageDisplayService,
        impl ImageRepository,
        impl DeparturesService,
    >,
> {
    assemble(
        config,
        ChromeRenderDisplayImageGenerator::new(cache.chrome(), chrome_idle_timeout, chrome_source),
        setup_display(&config.display, images),
        FileStoreImageRepository::new(config.file_store.save_directory.clone()),
    )
}
