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
            .query_param("daily", "temperature_2m_max,temperature_2m_min,uv_index_max")
            .query_param("forecast_days", "1");
        then.status(200)
            .header("content-type", "application/json; charset=UTF-8")
            .body(
                r#"{ "utc_offset_seconds": 3600, "current": { "temperature_2m": 3.4, "weather_code": 61 },
                     "daily": { "temperature_2m_max": [6.0], "temperature_2m_min": [1.6] } }"#,
            );
    });

    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), server.base_url(), reqwest::Client::new());

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

    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), server.base_url(), reqwest::Client::new());

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

    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), server.base_url(), reqwest::Client::new());

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

    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), server.base_url(), reqwest::Client::new());

    let weather = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await
        .unwrap()
        .unwrap();

    assert_that(&serde_json::to_value(&weather).unwrap()["precipitation"].is_null()).is_true();
}

#[tokio::test]
async fn lists_air_quality_worst_pollutant_first() {
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
    let air = server.mock(|when, then| {
        when.method(GET)
            .path("/v1/air-quality")
            .query_param("latitude", "1")
            .query_param("longitude", "2");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{ "current": { "european_aqi": 62, "european_aqi_pm2_5": 31, "european_aqi_pm10": 62,
                                  "european_aqi_nitrogen_dioxide": 27, "european_aqi_ozone": 18,
                                  "european_aqi_sulphur_dioxide": null } }"#,
            );
    });

    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), server.base_url(), reqwest::Client::new());

    let weather = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await
        .unwrap()
        .unwrap();

    air.assert();
    let air = serde_json::to_value(&weather).unwrap()["air_quality"].clone();
    assert_that(&air["overall"]["band"].as_str()).is_equal_to(Some("Poor"));
    let labels: Vec<_> = air["pollutants"].as_array().unwrap().iter().map(|p| p["label"].as_str().unwrap()).collect();
    assert_that(&labels).is_equal_to(vec!["PM10", "PM2.5", "NO₂", "Ozone"]);
}

#[tokio::test]
async fn weather_still_shows_when_air_quality_fails() {
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
    server.mock(|when, then| {
        when.method(GET).path("/v1/air-quality");
        then.status(500);
    });

    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), server.base_url(), reqwest::Client::new());

    let weather = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await
        .unwrap()
        .unwrap();

    assert_that(&serde_json::to_value(&weather).unwrap()["air_quality"].is_null()).is_true();
}

async fn weather_with_daily(daily: &str) -> serde_json::Value {
    let server = MockServer::start();
    let body = format!(
        r#"{{ "utc_offset_seconds": 0,
              "current": {{ "temperature_2m": 3.4, "weather_code": 0 }},
              "daily": {daily} }}"#
    );
    server.mock(|when, then| {
        when.method(GET).path("/v1/forecast");
        then.status(200).header("content-type", "application/json").body(body);
    });
    let under_test = OpenMeteoWeatherServiceAdapter::new(server.base_url(), server.base_url(), reqwest::Client::new());
    let weather = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await
        .unwrap()
        .unwrap();
    serde_json::to_value(&weather).unwrap()
}

#[tokio::test]
async fn shows_the_days_peak_uv_when_it_is_worth_noticing() {
    let weather = weather_with_daily(
        r#"{ "temperature_2m_max": [6.0], "temperature_2m_min": [1.6], "uv_index_max": [6.4] }"#,
    )
    .await;
    assert_that(&weather["uv_index"]["value"].as_u64()).is_equal_to(Some(6));
    assert_that(&weather["uv_index"]["band"].as_str()).is_equal_to(Some("High"));
}

#[tokio::test]
async fn shows_low_uv_but_leaves_it_off_when_missing() {
    let low = weather_with_daily(
        r#"{ "temperature_2m_max": [6.0], "temperature_2m_min": [1.6], "uv_index_max": [2.2] }"#,
    )
    .await;
    assert_that(&low["uv_index"]["value"].as_u64()).is_equal_to(Some(2));
    assert_that(&low["uv_index"]["band"].as_str()).is_equal_to(Some("Low"));
    let missing = weather_with_daily(
        r#"{ "temperature_2m_max": [6.0], "temperature_2m_min": [1.6] }"#,
    )
    .await;
    assert_that(&missing["uv_index"].is_null()).is_true();
}
