use httpmock::prelude::*;
use speculoos::prelude::*;

use crate::adapters::stop_points::tfl::tfl_stop_point_service::TflStopPointServiceAdapter;
use crate::domain::models::arrival::Arrival;
use crate::domain::models::location::Location;
use crate::domain::models::stop_point::{StopKind, StopPoint};
use crate::domain::services::arrivals_service::ArrivalsService;
use crate::domain::services::stop_point_service::StopPointService;

const NEARBY_RESPONSE: &str = r#"{
    "stopPoints": [
        {
            "naptanId": "490000002B", "commonName": "Far Road", "indicator": "Stop B",
            "distance": 150.5, "lat": 51.58, "lon": -0.11,
            "lines": [{ "id": "w3", "name": "W3" }, { "id": "41", "name": "41" }]
        },
        {
            "naptanId": "490000001A", "commonName": "Near Road", "indicator": "",
            "distance": 20.0, "lat": 51.59, "lon": -0.12, "lines": []
        }
    ]
}"#;

fn adapter(server: &MockServer) -> TflStopPointServiceAdapter {
    TflStopPointServiceAdapter::new(server.base_url(), None, reqwest::Client::new())
}

#[tokio::test]
async fn finds_nearby_bus_stops_sorted_by_distance() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/StopPoint")
            .query_param("lat", "51.586572")
            .query_param("lon", "-0.112116")
            .query_param("stopTypes", "NaptanPublicBusCoachTram")
            .query_param("radius", "200");
        then.status(200)
            .header("content-type", "application/json")
            .body(NEARBY_RESPONSE);
    });

    let result = adapter(&server)
        .find_nearby(Location::new(51.586572, -0.112116), StopKind::Bus, 200)
        .await;

    mock.assert();
    assert_that(&result).is_ok_containing(vec![
        StopPoint::new(
            "490000001A".into(),
            "Near Road".into(),
            None,
            Some(20.0),
            vec![],
            51.59,
            -0.12,
        ),
        StopPoint::new(
            "490000002B".into(),
            "Far Road".into(),
            Some("Stop B".into()),
            Some(150.5),
            vec!["W3".into(), "41".into()],
            51.58,
            -0.11,
        ),
    ]);
}

#[tokio::test]
async fn finds_nearby_underground_stations_using_metro_stop_type() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/StopPoint")
            .query_param("stopTypes", "NaptanMetroStation");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"stopPoints": []}"#);
    });

    let result = adapter(&server)
        .find_nearby(Location::new(51.5, -0.1), StopKind::Underground, 1000)
        .await;

    mock.assert();
    assert_that(&result).is_ok_containing(vec![]);
}

#[tokio::test]
async fn searches_underground_stations_by_name() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/StopPoint/Search")
            .query_param("query", "finsbury")
            .query_param("modes", "tube");
        then.status(200).header("content-type", "application/json").body(
            r#"{"matches": [{"id": "940GZZLUFPK", "name": "Finsbury Park", "lat": 51.564, "lon": -0.106}]}"#,
        );
    });

    let result = adapter(&server)
        .search_by_name("finsbury", StopKind::Underground)
        .await;

    mock.assert();
    assert_that(&result).is_ok_containing(vec![StopPoint::new(
        "940GZZLUFPK".into(),
        "Finsbury Park".into(),
        None,
        None,
        vec![],
        51.564,
        -0.106,
    )]);
}

#[tokio::test]
async fn returns_arrivals_soonest_first() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/StopPoint/940GZZLUTPN/Arrivals");
        then.status(200).header("content-type", "application/json").body(
            r#"[
                {"lineName": "Piccadilly", "destinationName": "Cockfosters Underground Station",
                 "towards": "Cockfosters", "platformName": "EastBound - Platform 1",
                 "currentLocation": "Between Arsenal and Finsbury Park", "timeToStation": 343,
                 "stationName": "Turnpike Lane Underground Station"},
                {"lineName": "12", "towards": "Marble Arch", "platformName": "RC", "timeToStation": 60}
            ]"#,
        );
    });

    let result = adapter(&server).get_arrivals("940GZZLUTPN").await;

    mock.assert();
    assert_that(&result).is_ok_containing(vec![
        Arrival::new(
            "12".into(),
            "".into(),
            "Marble Arch".into(),
            "RC".into(),
            "".into(),
            60,
            "".into(),
        ),
        Arrival::new(
            "Piccadilly".into(),
            "Cockfosters Underground Station".into(),
            "Cockfosters".into(),
            "EastBound - Platform 1".into(),
            "Between Arsenal and Finsbury Park".into(),
            343,
            "Turnpike Lane Underground Station".into(),
        ),
    ]);
}
