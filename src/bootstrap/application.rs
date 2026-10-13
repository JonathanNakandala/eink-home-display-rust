use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::adapters::departures::DeparturesServiceImpl;
use crate::adapters::display_image_generator::chrome_render::{
    ChromeRenderDisplayImageGenerator, ChromeSource, Templates,
};
use crate::adapters::image_repository::file_store::FileStoreImageRepository;
use crate::adapters::weather::WeatherServiceImpl;
use crate::application::{Application, RenderPolicy};
use crate::config::application::{ApplicationConfig, LayoutsConfig};
use crate::config::cache::CachePaths;
use crate::domain::models::layout::Layouts;
use crate::domain::services::ImageDisplayService;
use crate::domain::services::clock::Clock;
use crate::domain::services::departures_service::DeparturesService;
use crate::domain::services::display_image_generator::DisplayImageGenerator;
use crate::domain::services::image_repository::ImageRepository;
use crate::domain::services::published_images::PublishedImages;
use crate::domain::services::weather_service::WeatherService;

use super::{setup_departure_boards, setup_display, setup_weather_service};

/// The application with the data sources the configuration asks for and the three ports that
/// differ between the programs that use it: how the image is drawn, shown and stored. `clock`
/// is where the application reads the time.
pub fn assemble<DIG, IDS, IR>(
    config: &ApplicationConfig,
    generator: DIG,
    display: IDS,
    repository: IR,
    clock: Arc<dyn Clock>,
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
        RenderPolicy {
            max_age: (&config.stale_data).into(),
            limits: (&config.limits).into(),
        },
        clock,
    ))
}

/// The layouts the service draws and how to choose between them.
pub struct LayoutSet {
    /// The page behind each layout.
    pub templates: Templates,
    /// The layouts those make, with the configured schedule of which one a display is sent when.
    pub layouts: Layouts,
}

/// The built-in templates and those in the configured folder (`directory`, if given, is used in place of it), each
/// checked, and the schedule over the layouts they make. A template that can't be drawn, or a rule for a layout there
/// isn't or that can't be a rule, is an error naming it.
pub fn load_layouts(config: &LayoutsConfig, directory: Option<&Path>) -> anyhow::Result<LayoutSet> {
    let templates = Templates::built_in();
    let templates = match directory.or(config.directory.as_deref()) {
        Some(directory) => templates.with_directory(directory)?,
        None => templates,
    };
    let schedule = config.to_schedule().map_err(anyhow::Error::msg)?;
    let layouts = templates
        .layouts()
        .with_schedule(schedule)
        .map_err(anyhow::Error::msg)?;
    Ok(LayoutSet { templates, layouts })
}

/// The service as it runs: rendered by Chrome, shown on the configured display, and saved to the
/// configured folder. `images` is where displays that fetch their image have it published, and `templates` are the
/// layouts it draws (see `load_layouts`).
pub fn from_config(
    config: &ApplicationConfig,
    cache: &CachePaths,
    chrome_idle_timeout: Duration,
    chrome_source: ChromeSource,
    images: Arc<dyn PublishedImages>,
    layouts: LayoutSet,
    clock: Arc<dyn Clock>,
) -> anyhow::Result<
    Application<
        impl WeatherService,
        impl DisplayImageGenerator,
        impl ImageDisplayService,
        impl ImageRepository,
        impl DeparturesService,
    >,
> {
    Ok(assemble(
        config,
        ChromeRenderDisplayImageGenerator::new(cache.chrome(), chrome_idle_timeout, chrome_source)
            .with_templates(layouts.templates),
        setup_display(&config.display, images),
        FileStoreImageRepository::new(config.file_store.save_directory.clone()),
        clock,
    )?
    .with_layouts(layouts.layouts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::application::LayoutRuleConfig;

    fn rule(layout: &str) -> LayoutRuleConfig {
        LayoutRuleConfig {
            layout: layout.to_owned(),
            days: vec![],
            from: "06:00".to_owned(),
            until: "07:00".to_owned(),
        }
    }

    #[test]
    fn a_schedule_for_layouts_there_are_is_loaded() {
        let config = LayoutsConfig {
            directory: None,
            schedule: vec![rule("departures")],
        };

        let layouts = load_layouts(&config, None).unwrap().layouts;

        assert_eq!(layouts.schedule().rules().len(), 1);
    }

    #[test]
    fn a_rule_for_a_layout_there_is_not_stops_the_start_naming_what_there_is() {
        let config = LayoutsConfig {
            directory: None,
            schedule: vec![rule("nonesuch")],
        };

        let error = load_layouts(&config, None).err().unwrap();

        let message = format!("{error:#}");
        assert!(message.contains("nonesuch"), "{message}");
        assert!(message.contains("dashboard, departures"), "{message}");
    }

    #[test]
    fn a_rule_can_name_a_layout_that_comes_from_the_templates_folder() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("clock.html"),
            "<body>{{date.currentTime}}</body>",
        )
        .unwrap();
        let config = LayoutsConfig {
            directory: Some(directory.path().to_path_buf()),
            schedule: vec![rule("clock")],
        };

        let loaded = load_layouts(&config, None).unwrap();

        assert_eq!(loaded.layouts.schedule().rules().len(), 1);
        assert!(loaded.layouts.find("clock").is_some());
    }

    #[test]
    fn a_badly_written_rule_is_an_error_that_says_where() {
        let mut bad = rule("departures");
        bad.from = "6am".to_owned();
        let config = LayoutsConfig {
            directory: None,
            schedule: vec![bad],
        };

        let error = load_layouts(&config, None).err().unwrap();

        assert!(format!("{error:#}").contains("[[layouts.schedule]] number 1"));
    }
}
