use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;

use eink_home_display_rust::adapters::clock::SystemClock;
use eink_home_display_rust::adapters::display_image_generator::chrome_render::{
    ChromeSource, DEFAULT_IDLE_TIMEOUT,
};
use eink_home_display_rust::adapters::image_server::{Handles, ServerSettings, router};
use eink_home_display_rust::adapters::published_images::DirectoryImages;
use eink_home_display_rust::application::devices::DeviceBoard;
use eink_home_display_rust::application::launch;
use eink_home_display_rust::application::refresh::RefreshControl;
use eink_home_display_rust::application::status::StatusBoard;
use eink_home_display_rust::bootstrap;
use eink_home_display_rust::cli;
use eink_home_display_rust::config::cache::CachePaths;
use eink_home_display_rust::domain::models::location::Location;
use eink_home_display_rust::domain::services::clock::Clock;
use eink_home_display_rust::scheduler::{
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

    let configured = config
        .schedule
        .as_ref()
        .map(|schedule| schedule.to_schedule())
        .transpose()?;
    let Some(schedule) = args.schedule(configured)? else {
        return bootstrap::from_config(
            &config,
            &cache,
            DEFAULT_IDLE_TIMEOUT,
            chrome_source,
            Arc::new(DirectoryImages::new(config.server.directory.clone())),
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
        && launch::served_image_is_current(
            images.as_ref(),
            config.display.image_format.into(),
            &schedule,
            now,
        )
        .await;
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
        clock.clone(),
    )?
    .with_observer(Arc::new(launch::AttemptMarker::new(marker, clock.clone())))
    // Status before refresh: a button press is answered as soon as the refresh hears the render
    // end, and whoever then reads /status must see that render's outcome.
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
    if !config.display.kind.fetches_image() {
        return periodic.await;
    }
    // The display downloads its image, so serve it for as long as the refresh loop runs.
    let settings = ServerSettings::from(&config.server);
    let format = config.display.image_format.into();
    // The certificate authority and the list of displays are opened first, so that a missing or unreadable
    // one stops the program here, and so `/status` can report on them.
    let security = bootstrap::open_security(&config.server, clock.clone(), args.init_pki).await?;
    let display_routes = router(
        images,
        format,
        schedule.clone(),
        settings.timing,
        Handles {
            refresh: refresh.clone(),
            status: status.clone(),
            devices: devices.clone(),
            members: security.as_ref().map(|s| s.enrollment.clone()),
            handshakes: security.as_ref().map(|s| s.handshakes.clone()),
        },
        clock.clone(),
    );
    // Opened before anything runs, so a port that is taken stops the program here too.
    let listening =
        bootstrap::start_serving(&config.server, display_routes, format, security).await?;
    tokio::select! {
        result = periodic => result,
        result = listening.run() => result,
    }
}
