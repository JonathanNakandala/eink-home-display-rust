//! The composition root: reads the configuration and builds the adapters it asks for.
//! Adapters take plain values; only this module knows both the config and the adapters.

mod departures;
mod display;
mod weather;

pub use self::departures::setup_departure_boards;
pub use self::display::setup_display;
pub use self::weather::setup_weather_service;
