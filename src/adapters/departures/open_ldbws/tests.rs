use httpmock::prelude::*;
use speculoos::prelude::*;

use crate::adapters::departures::open_ldbws::open_ldbws_departures_service::OpenLdbwsDeparturesServiceAdapter;
use crate::domain::models::departures::DepartureService;
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
        "apikey".to_owned(),
        reqwest::Client::new(),
    );

    let result = under_test.get_departures("HRN", "WGC", 4).await;

    mock.assert();
    assert_that(&result).is_ok_containing(vec![
        DepartureService::new(
            "18:04".to_owned(),
            "Welwyn Garden City".to_owned(),
            "On time".to_owned(),
            String::new(),
        ),
        DepartureService::new(
            "18:19".to_owned(),
            "Welwyn Garden City".to_owned(),
            "Delayed".to_owned(),
            "18:27".to_owned(),
        ),
        DepartureService::new(
            "18:34".to_owned(),
            "Welwyn Garden City".to_owned(),
            "Cancelled".to_owned(),
            String::new(),
        ),
    ]);
}
