//! The composition root: reads the configuration and builds the adapters it asks for.
//! Adapters take plain values; only this module knows both the config and the adapters.
//! The programs in `src/bin` and `main` are thin: they parse their arguments, call into here,
//! and run.

mod application;
mod departures;
mod display;
mod load;
mod logging;
mod serving;
mod weather;
mod zone;

pub use self::application::{assemble, from_config};
pub use self::departures::setup_departure_boards;
pub use self::display::setup_display;
pub use self::load::{
    load_application_config, load_quiet_times_config, load_valid_application_config,
};
pub use self::logging::init_logging;
pub use self::serving::{Listening, Security, open_security, start as start_serving};
pub use self::weather::setup_weather_service;
pub use self::zone::{configured_zone, resolve_zone};
