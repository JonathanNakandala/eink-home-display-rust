use httpmock::prelude::*;
use speculoos::prelude::*;

use crate::adapters::weather::open_weather::open_weather_weather_service::OpenWeatherWeatherServiceAdapter;
use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;
use crate::domain::services::weather_service::WeatherService;

#[tokio::test]
async fn returns_temperature_at_location() {
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
            .body("{ \"main\": { \"temp\": 3.0 } }");
    });

    let under_test = OpenWeatherWeatherServiceAdapter::new(
        server.base_url(),
        "apikey".to_owned(),
        reqwest::Client::new(),
    );

    let result = under_test
        .get_weather_for_location(Location::new(1f64, 2f64))
        .await;

    hello_mock.assert();
    assert_that(&result).is_ok_containing(WeatherInformation::new(3));
}
