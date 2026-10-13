use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::adapters::departures::DeparturesServiceImpl;
use crate::adapters::display_image_generator::chrome_render::{
    ChromeRenderDisplayImageGenerator, ChromeSource, Templates,
};
use crate::adapters::image_display_service::web_screen::WebScreenAdapter;
use crate::adapters::image_repository::file_store::FileStoreImageRepository;
use crate::adapters::published_images::DirectoryImages;
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
    let mut application = assemble(
        config,
        ChromeRenderDisplayImageGenerator::new(cache.chrome(), chrome_idle_timeout, chrome_source)
            .with_templates(layouts.templates),
        setup_display(&config.display, images),
        FileStoreImageRepository::new(config.file_store.save_directory.clone()),
        clock,
    )?
    .with_layouts(layouts.layouts);
    // The page for a browser is another output, drawn for besides the display, when it has a screen of its own.
    if let Some(output) = setup_web_screen(&config.server) {
        application = application.with_output(output);
    }
    Ok(application)
}

/// Where the page's own pictures are kept, if it has a screen of its own.
pub fn web_screen_images(
    config: &crate::config::server::ServerConfig,
) -> Option<Arc<dyn PublishedImages>> {
    config
        .web
        .screen
        .as_ref()
        .filter(|_| config.web.enabled)
        .map(|_| {
            Arc::new(DirectoryImages::new(config.web_images_directory()))
                as Arc<dyn PublishedImages>
        })
}

/// The page for a browser as an output, if `[server.web]` is on and has a `screen`.
pub fn setup_web_screen(
    config: &crate::config::server::ServerConfig,
) -> Option<Arc<dyn ImageDisplayService>> {
    let screen = config.web.screen.as_ref().filter(|_| config.web.enabled)?;
    Some(Arc::new(WebScreenAdapter::new(
        screen.width,
        screen.height,
        Arc::new(DirectoryImages::new(config.web_images_directory())),
    )))
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

    fn server(toml_text: &str) -> crate::config::server::ServerConfig {
        toml::from_str(toml_text).unwrap()
    }

    #[test]
    fn the_page_is_an_output_only_when_it_is_on_and_has_a_screen_of_its_own() {
        for off in [
            "",
            "[web]\nenabled = true\nopen = true",
            "[web]\nopen = true",
        ] {
            assert!(setup_web_screen(&server(off)).is_none(), "{off}");
            assert!(web_screen_images(&server(off)).is_none(), "{off}");
        }

        let on =
            server("[web]\nenabled = true\nopen = true\n[web.screen]\nwidth = 1280\nheight = 800");
        let output = setup_web_screen(&on).expect("an output");

        assert_eq!(output.name(), "web");
        let profile = output.profile();
        assert_eq!((profile.width, profile.height), (1280, 800));
        assert!(output.wants_every_layout());
        assert!(web_screen_images(&on).is_some());
    }

    #[tokio::test]
    async fn what_is_drawn_for_the_page_goes_to_its_own_folder_and_not_the_panels() {
        use crate::domain::models::display::ImageFormat;
        use crate::domain::models::image::ImageData;
        use crate::domain::models::layout::LayoutName;
        use std::io::Cursor;

        let tmp = tempfile::tempdir().unwrap();
        let config = server(&format!(
            "directory = {:?}\n[web]\nenabled = true\nopen = true\n[web.screen]\nwidth = 32\nheight = 16",
            tmp.path()
        ));
        let mut png = Cursor::new(Vec::new());
        image::RgbImage::new(32, 16)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let layout = LayoutName::parse("dashboard").unwrap();

        setup_web_screen(&config)
            .unwrap()
            .display(&layout, &ImageData::new(png.into_inner()))
            .await
            .unwrap();

        assert!(
            tmp.path()
                .join("_web")
                .join("dashboard")
                .join("image.png")
                .exists()
        );
        // Nothing among the panel's own (the layout folders directly under the directory).
        assert!(!tmp.path().join("dashboard").exists());
        let panel = DirectoryImages::new(tmp.path());
        assert!(
            panel
                .published_at(&layout, ImageFormat::Png)
                .await
                .unwrap()
                .is_none()
        );
    }
}
