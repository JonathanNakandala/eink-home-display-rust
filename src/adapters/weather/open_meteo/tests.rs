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
                r#"{ "utc_offset_seconds": 3600, "current": { "temperature_2m": 3.4, "weather_code": 61 },
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
                r#"{ "utc_offset_seconds": 3600, "current": { "temperature_2m": 3.4, "weather_code": 0 },
                     "daily": { "temperature_2m_max": [], "temperature_2m_min": [] } }"#,
            );
    });

    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), reqwest::Client::new());

    let result = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await;

    assert_that(&result).is_err();
}

#[tokio::test]
async fn charts_the_coming_precipitation_when_some_is_expected() {
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/v1/forecast")
            .query_param("minutely_15", "precipitation,snowfall,weather_code")
            .query_param("forecast_minutely_15", "8");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{ "utc_offset_seconds": 3600,
                     "current": { "temperature_2m": 12.0, "weather_code": 3 },
                     "daily": { "temperature_2m_max": [14.0], "temperature_2m_min": [9.0] },
                     "minutely_15": {
                       "time": ["2099-10-01T11:15", "2099-10-01T11:30", "2099-10-01T11:45", "2099-10-01T12:00",
                                "2099-10-01T12:15", "2099-10-01T12:30", "2099-10-01T12:45", "2099-10-01T13:00"],
                       "precipitation": [0.0, 0.0, 0.0, 0.1, 0.2, 0.0, null, 0.0],
                       "snowfall": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                       "weather_code": [3, 3, 3, 51, 53, 3, 3, 3] } }"#,
            );
    });

    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), reqwest::Client::new());

    let weather = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await
        .unwrap()
        .unwrap();

    mock.assert();
    let chart = serde_json::to_value(&weather).unwrap()["precipitation"].clone();
    assert_that(&chart["caption"].as_str()).is_equal_to(Some("UPCOMING DRIZZLE"));
    assert_that(&chart["start_label"].as_str()).is_equal_to(Some("12:00"));
    // 0.2 mm in 15 minutes is 0.8 mm/h.
    assert_that(&chart["peak_label"].as_str()).is_equal_to(Some("0.8 mm/h"));
}

#[tokio::test]
async fn leaves_the_chart_off_when_the_forecast_has_no_precipitation_data() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v1/forecast");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{ "utc_offset_seconds": 0,
                     "current": { "temperature_2m": 3.4, "weather_code": 0 },
                     "daily": { "temperature_2m_max": [6.0], "temperature_2m_min": [1.6] } }"#,
            );
    });

    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), reqwest::Client::new());

    let weather = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await
        .unwrap()
        .unwrap();

    assert_that(&serde_json::to_value(&weather).unwrap()["precipitation"].is_null()).is_true();
}
