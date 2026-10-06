use chrono::{DateTime, Local, TimeZone};
use httpmock::prelude::*;
use speculoos::prelude::*;

use crate::adapters::departures::open_ldbws::open_ldbws_departures_service::OpenLdbwsDeparturesServiceAdapter;
use crate::domain::models::departures::{DepartureService, DepartureStatus, Departures};
use crate::domain::services::departures_service::DeparturesService;

const SAMPLE_RESPONSE: &str = r#"{
    "generatedAt": "2024-01-10T18:00:00+00:00",
    "locationName": "Hornsey",
    "crs": "HRN",
    "trainServices": [
        {
            "std": "18:04",
            "etd": "On time",
            "destination": [{ "locationName": "Welwyn Garden City", "crs": "WGC" }]
        },
        {
            "std": "18:19",
            "etd": "18:27",
            "destination": [{ "locationName": "Welwyn Garden City", "crs": "WGC" }]
        },
        {
            "std": "18:34",
            "etd": "Cancelled",
            "destination": [{ "locationName": "Welwyn Garden City", "crs": "WGC" }]
        },
        {
            "sta": "18:40",
            "eta": "On time",
            "origin": [{ "locationName": "Moorgate", "crs": "MOG" }]
        }
    ]
}"#;

/// The sample board was generated at 18:00.
fn now() -> DateTime<Local> {
    Local.with_ymd_and_hms(2024, 1, 10, 18, 0, 0).unwrap()
}

#[tokio::test]
async fn returns_parsed_departures_for_station_pair() {
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/GetArrDepBoardWithDetails/HRN")
            .header("x-apikey", "apikey")
            .query_param("numRows", "4")
            .query_param("filterCrs", "WGC")
            .query_param("filterType", "to");
        then.status(200)
            .header("content-type", "application/json")
            .body(SAMPLE_RESPONSE);
    });

    let under_test = OpenLdbwsDeparturesServiceAdapter::new(
        format!("{}/GetArrDepBoardWithDetails", server.base_url()),
        "apikey".into(),
        "HRN".to_owned(),
        "WGC".to_owned(),
        0,
        reqwest::Client::new(),
    );

    let result = under_test.get_departures(4, now()).await;

    mock.assert();
    assert_that(&result).is_ok_containing(Departures::new(
        "Hornsey".to_owned(),
        vec![
        DepartureService::new(
            "18:04".to_owned(),
            "Welwyn Garden City".to_owned(),
            DepartureStatus::OnTime,
            String::new(),
            "4 min".to_owned(),
        ),
        DepartureService::new(
            "18:19".to_owned(),
            "Welwyn Garden City".to_owned(),
            DepartureStatus::Delayed,
            "18:27".to_owned(),
            "27 min".to_owned(),
        ),
        DepartureService::new(
            "18:34".to_owned(),
            "Welwyn Garden City".to_owned(),
            DepartureStatus::Cancelled,
            String::new(),
            String::new(),
        ),
    ],
    ));
}

#[tokio::test]
async fn asks_for_the_board_as_it_will_be_after_the_travel_time() {
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/GetArrDepBoardWithDetails/HRN")
            .query_param("timeOffset", "5");
        then.status(200)
            .header("content-type", "application/json")
            .body(SAMPLE_RESPONSE);
    });

    let under_test = OpenLdbwsDeparturesServiceAdapter::new(
        format!("{}/GetArrDepBoardWithDetails", server.base_url()),
        "apikey".into(),
        "HRN".to_owned(),
        "WGC".to_owned(),
        5,
        reqwest::Client::new(),
    );

    under_test.get_departures(4, now()).await.unwrap();

    mock.assert();
}

#[tokio::test]
async fn caps_the_time_offset_at_what_the_api_accepts() {
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/GetArrDepBoardWithDetails/HRN")
            .query_param("timeOffset", "119");
        then.status(200)
            .header("content-type", "application/json")
            .body(SAMPLE_RESPONSE);
    });

    let under_test = OpenLdbwsDeparturesServiceAdapter::new(
        format!("{}/GetArrDepBoardWithDetails", server.base_url()),
        "apikey".into(),
        "HRN".to_owned(),
        "WGC".to_owned(),
        500,
        reqwest::Client::new(),
    );

    under_test.get_departures(4, now()).await.unwrap();

    mock.assert();
}
