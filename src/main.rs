use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;

use home_display_server::adapters::clock::SystemClock;
use home_display_server::adapters::display_image_generator::chrome_render::{
    ChromeSource, DEFAULT_IDLE_TIMEOUT,
};
use home_display_server::adapters::image_server::{Handles, ServerSettings, router};
use home_display_server::adapters::published_images::DirectoryImages;
use home_display_server::application::devices::DeviceBoard;
use home_display_server::application::launch;
use home_display_server::application::refresh::RefreshControl;
use home_display_server::application::status::StatusBoard;
use home_display_server::bootstrap;
use home_display_server::cli;
use home_display_server::config::cache::CachePaths;
use home_display_server::domain::models::location::Location;
use home_display_server::domain::services::clock::Clock;
use home_display_server::scheduler::{
    PERIODIC_IDLE_TIMEOUT, log_schedule, run_periodically_from, shutdown_signal,
};

#[tokio::main]
async fn main() -> Result<()> {
    bootstrap::init_logging("debug");
    // TODO: Figure out how to log the current log level as RUST_LOG should by able to override via try_from_default_env
    tracing::info!(
        "Logging initialized at {} level",
        tracing::level_filters::STATIC_MAX_LEVEL
    );

    let args = match cli::Args::try_parse() {
        Ok(args) => args,
        Err(e) => {
            log::error!("Failed to parse command-line arguments: {}", e);
            eprintln!("Error: Invalid command-line arguments. Use --help for usage information.");
            std::process::exit(1);
        }
    };

    let config = bootstrap::load_valid_application_config(&args.config_file)?;

    log::info!("Settings loaded successfully: {:?}", config);

    let location = Location::new(config.location.latitude, config.location.longitude);
    let cache = CachePaths::new(
        args.cache_dir
            .clone()
            .unwrap_or_else(|| config.cache.directory.clone()),
    );
    cache.ensure_exists()?;
    let chrome_source = ChromeSource::from(args.bundled_chrome);
    let zone = bootstrap::resolve_zone(&config.location)?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new(zone));

    // The keys for other services: the configuration file's, and those the owner sets while this runs.
    let credentials = bootstrap::setup_credentials(&config, clock.clone());

    // Read and checked before anything runs, so a template with a mistake in it stops the program here.
    let layouts = bootstrap::load_layouts(&config.layouts, None)?;
    let served_layouts = layouts.layouts.clone();
    let configured = config
        .schedule
        .as_ref()
        .map(|schedule| schedule.to_schedule())
        .transpose()?;
    if args.check_config {
        // The flags are checked against the file's schedule as a run would, and an authority is made if asked.
        args.schedule(configured.clone())?;
        bootstrap::open_security(&config.server, clock.clone(), credentials, args.init_pki).await?;
        println!(
            "The configuration in {} is valid",
            args.config_file.display()
        );
        return Ok(());
    }
    let Some(schedule) = args.schedule(configured)? else {
        return bootstrap::from_config(
            &config,
            &cache,
            DEFAULT_IDLE_TIMEOUT,
            chrome_source,
            Arc::new(DirectoryImages::new(config.server.directory.clone())),
            layouts,
            credentials,
            clock,
        )?
        .run(location)
        .await
        .map(|_| ());
    };
    let now = clock.now();
    log_schedule(&schedule, now);
    let images = Arc::new(DirectoryImages::new(config.server.directory.clone()));
    let marker = cache.render_attempt();
    let image_is_current = config.display.kind.fetches_image()
        && launch::served_images_are_current(
            images.as_ref(),
            &served_layouts,
            config.display.image_format.into(),
            &schedule,
            now,
        )
        .await
        // The page's own pictures too, if it has them: one missing or due is a reason to render.
        && match bootstrap::web_screen_images(&config.server) {
            Some(web_images) => {
                launch::served_images_are_current(
                    web_images.as_ref(),
                    &served_layouts,
                    home_display_server::domain::models::display::ImageFormat::Png,
                    &schedule,
                    now,
                )
                .await
            }
            None => true,
        };
    let first = launch::first_render_at(
        now,
        args.no_initial_run,
        image_is_current,
        launch::last_attempt(&marker),
        args.restart_cooldown,
    );
    match first {
        Some(at) if at > now => log::info!(
            "Holding the first render until {} (restart cooldown)",
            at.format("%H:%M:%S")
        ),
        Some(_) => log::info!("Rendering now"),
        None if image_is_current => {
            log::info!("The served image is current; waiting for the next slot")
        }
        None => {}
    }
    let refresh = RefreshControl::new(Duration::from_secs(
        config.server.refresh_cooldown_seconds.into(),
    ));
    let status = StatusBoard::new(now);
    let devices = DeviceBoard::new(Duration::from_secs(
        config.server.device_overdue_grace_seconds.into(),
    ));
    // Built once so the Chrome it launches is kept between runs.
    let app = bootstrap::from_config(
        &config,
        &cache,
        PERIODIC_IDLE_TIMEOUT,
        chrome_source,
        images.clone(),
        layouts,
        credentials.clone(),
        clock.clone(),
    )?
    .with_observer(Arc::new(launch::AttemptMarker::new(marker, clock.clone())))
    // Status before refresh: a button press is answered as soon as the refresh hears that the display's picture is drawn (or, if
    // the run fails first, that it has ended), which is before the outputs besides the display are; so /status may not yet show
    // that run's outcome when the display has its answer, and does once the run ends.
    .with_observer(status.clone())
    .with_observer(refresh.clone());
    let periodic = run_periodically_from(
        &schedule,
        first,
        refresh.wake(),
        clock.as_ref(),
        shutdown_signal(),
        || {
            let run = app.run(location);
            async move { run.await.map(|_| ()) }
        },
    );
    // Served for a display that downloads its image, and for the page for a browser, for as long as the refresh loop runs.
    if !config.display.kind.fetches_image() && !config.server.web.enabled {
        bootstrap::report_missing_keys(&config, &credentials);
        return periodic.await;
    }
    let settings = ServerSettings::from(&config.server);
    let format = config.display.image_format.into();
    // The certificate authority and the list of displays are opened first, so that a missing or unreadable
    // one stops the program here, and so `/status` can report on them.
    let security = bootstrap::open_security(
        &config.server,
        clock.clone(),
        credentials.clone(),
        args.init_pki,
    )
    .await?;
    // After the remembered keys are back, so that only what is really missing is said.
    bootstrap::report_missing_keys(&config, &credentials);
    let handles = Handles {
        layouts: served_layouts.clone(),
        refresh: refresh.clone(),
        status: status.clone(),
        devices: devices.clone(),
        layout_choices: security.as_ref().map(|s| s.layout_choices.clone()),
        viewers: security.as_ref().map(|s| s.viewers.clone()),
        members: security.as_ref().map(|s| s.enrollment.clone()),
        handshakes: security.as_ref().map(|s| s.handshakes.clone()),
    };
    let web = bootstrap::web_routes(
        &config.server,
        images.clone(),
        format,
        schedule.clone(),
        handles.clone(),
        clock.clone(),
    )?;
    // What the service is judged by: the panel's pictures, or, for a panel driven directly, the page's own.
    let (served, served_format) = bootstrap::served_images(&config, images, format);
    let display_routes = router(
        served,
        served_format,
        schedule.clone(),
        settings.timing,
        handles,
        clock.clone(),
    );
    // Opened before anything runs, so a port that is taken stops the program here too.
    let listening = bootstrap::start_serving(
        &config.server,
        display_routes,
        web,
        format,
        served_layouts,
        security,
    )
    .await?;
    tokio::select! {
        result = periodic => result,
        result = listening.run() => result,
    }
}
