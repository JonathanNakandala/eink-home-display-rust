use std::sync::Arc;

use chrono_tz::Tz;
use httpmock::prelude::*;
use speculoos::prelude::*;

use crate::adapters::clock::SystemClock;
use crate::adapters::weather::open_weather::open_weather_weather_service::OpenWeatherWeatherServiceAdapter;
use crate::application::credentials::Credentials;
use crate::domain::models::credential::CredentialName;
use crate::domain::models::location::Location;
use crate::domain::models::source_error::SourceError;
use crate::domain::models::weather::{WeatherCondition, WeatherInformation};
use crate::domain::services::credential_source::{CredentialSource, FixedCredentials};
use crate::domain::services::weather_service::WeatherService;

fn fixed_key() -> Arc<dyn CredentialSource> {
    Arc::new(FixedCredentials::none().with(CredentialName::OpenWeather, "apikey"))
}

const SAMPLE: &str =
    r#"{ "main": { "temp": 3.4, "temp_min": 1.6, "temp_max": 6.0 }, "weather": [{ "id": 501 }] }"#;

#[tokio::test]
async fn without_a_key_no_request_is_made_and_the_source_says_so() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.any_request();
        then.status(200);
    });
    let under_test = OpenWeatherWeatherServiceAdapter::new(
        server.base_url(),
        Arc::new(FixedCredentials::none()),
        reqwest::Client::new(),
    );

    let result = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await;

    assert!(matches!(result, Err(SourceError::KeyNotSet)), "{result:?}");
    any.assert_calls(0);
}

#[tokio::test]
async fn a_key_set_after_the_service_was_made_is_used_from_the_next_request() {
    let server = MockServer::start();
    let with_key = server.mock(|when, then| {
        when.method(GET)
            .path("/data/2.5/weather")
            .query_param("appid", "0123456789abcdef0123456789abcdef");
        then.status(200)
            .header("content-type", "application/json")
            .body(SAMPLE);
    });
    let credentials = Credentials::new(Arc::new(SystemClock::new(Tz::UTC)), []);
    let under_test = OpenWeatherWeatherServiceAdapter::new(
        server.base_url(),
        credentials.clone(),
        reqwest::Client::new(),
    );
    let before = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await;
    assert!(matches!(before, Err(SourceError::KeyNotSet)));

    credentials
        .set(
            CredentialName::OpenWeather,
            "0123456789abcdef0123456789abcdef".to_owned().into(),
            false,
        )
        .await
        .unwrap();
    let after = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await;

    assert!(after.is_ok(), "{after:?}");
    with_key.assert();
}

#[tokio::test]
async fn returns_temperatures_and_condition_at_location() {
    let server = MockServer::start();

    let hello_mock = server.mock(|when, then| {
        when.method(GET)
            .path("/data/2.5/weather")
            .query_param("lat", "1")
            .query_param("lon", "2")
            .query_param("appid", "apikey")
            .query_param("units", "metric");
        then.status(200)
            .header("content-type", "application/json; charset=UTF-8")
            .body(r#"{ "main": { "temp": 3.4, "temp_min": 1.6, "temp_max": 6.0 }, "weather": [{ "id": 501 }] }"#);
    });

    let under_test = OpenWeatherWeatherServiceAdapter::new(
        server.base_url(),
        fixed_key(),
        reqwest::Client::new(),
    );

    let result = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await;

    hello_mock.assert();
    assert_that(&result).is_ok_containing(Some(WeatherInformation::new(
        3,
        2,
        6,
        WeatherCondition::Rain,
    )));
}
