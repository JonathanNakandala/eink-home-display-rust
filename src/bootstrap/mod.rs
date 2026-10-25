//! The composition root: reads the configuration and builds the adapters it asks for.
//! Adapters take plain values; only this module knows both the config and the adapters.
//! The programs in `src/bin` and `main` are thin: they parse their arguments, call into here,
//! and run.

mod application;
mod credentials;
mod departures;
mod display;
mod load;
mod logging;
mod serving;
mod telemetry;
mod weather;
mod zone;

pub use self::application::{
    LayoutSet, assemble, from_config, load_layouts, served_images, setup_web_screen,
    web_screen_images,
};
pub use self::credentials::{report_missing_keys, setup_credentials};
pub use self::departures::setup_departure_boards;
pub use self::display::setup_display;
pub use self::load::{
    load_application_config, load_quiet_times_config, load_valid_application_config,
};
pub use self::logging::{LogGuard, init_logging};
pub use self::serving::{Listening, Security, open_security, start as start_serving, web_routes};
pub use self::weather::setup_weather_service;
pub use self::zone::{configured_zone, resolve_zone};
