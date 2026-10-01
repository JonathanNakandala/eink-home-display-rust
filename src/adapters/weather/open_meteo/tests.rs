use httpmock::prelude::*;
use speculoos::prelude::*;

use crate::adapters::weather::open_meteo::open_meteo_weather_service::OpenMeteoWeatherServiceAdapter;
use crate::domain::models::location::Location;
use crate::domain::models::weather::{WeatherCondition, WeatherInformation};
use crate::domain::services::weather_service::WeatherService;

#[tokio::test]
async fn returns_temperatures_and_condition_at_location() {
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/v1/forecast")
            .query_param("latitude", "1")
            .query_param("longitude", "2")
            .query_param("current", "temperature_2m,weather_code")
            .query_param("daily", "temperature_2m_max,temperature_2m_min")
            .query_param("forecast_days", "1");
        then.status(200)
            .header("content-type", "application/json; charset=UTF-8")
            .body(
                r#"{ "current": { "temperature_2m": 3.4, "weather_code": 61 },
                     "daily": { "temperature_2m_max": [6.0], "temperature_2m_min": [1.6] } }"#,
            );
    });

    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), reqwest::Client::new());

    let result = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await;

    mock.assert();
    assert_that(&result).is_ok_containing(Some(WeatherInformation::new(
        3,
        2,
        6,
        WeatherCondition::Rain,
    )));
}

#[tokio::test]
async fn errors_when_the_daily_forecast_is_empty() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v1/forecast");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{ "current": { "temperature_2m": 3.4, "weather_code": 0 },
                     "daily": { "temperature_2m_max": [], "temperature_2m_min": [] } }"#,
            );
    });

    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), reqwest::Client::new());

    let result = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await;

    assert_that(&result).is_err();
}
