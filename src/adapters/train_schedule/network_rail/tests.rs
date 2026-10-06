use httpmock::prelude::*;
use speculoos::prelude::*;

use crate::adapters::train_schedule::network_rail::cif_feed;
use crate::adapters::train_schedule::network_rail::network_rail_train_schedule_service::NetworkRailTrainScheduleServiceAdapter;
use crate::adapters::train_schedule::network_rail::schedule_cache::{self, CachedScheduleRecord};
use crate::domain::services::train_schedule_service::TrainScheduleService;

#[tokio::test]
async fn download_full_schedule_authenticates_and_writes_body_to_disk() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/CifFileAuthenticate")
            .query_param("type", "CIF_ALL_FULL_DAILY")
            .query_param("day", "toc-full")
            .header_exists("authorization");
        then.status(200).body("raw schedule contents");
    });

    let dest = tempfile::NamedTempFile::new().unwrap();
    let client = reqwest::Client::new();
    cif_feed::download_full_schedule(
        &client,
        &format!("{}/CifFileAuthenticate", server.base_url()),
        "user",
        &"pass".into(),
        dest.path(),
    )
    .await
    .unwrap();

    mock.assert();
    let contents = std::fs::read_to_string(dest.path()).unwrap();
    assert_that!(contents).is_equal_to("raw schedule contents".to_owned());
}

#[tokio::test]
async fn adapter_resolves_passages_from_cache_file() {
    let cache_file = tempfile::NamedTempFile::new().unwrap();
    let records = vec![CachedScheduleRecord {
        uid: "UID1".to_owned(),
        start_date: "2024-01-01".to_owned(),
        end_date: "2024-12-31".to_owned(),
        days_runs: "1111111".to_owned(),
        stp_indicator: "P".to_owned(),
        minutes_past_midnight: Some(615.0),
    }];
    schedule_cache::save_cache(cache_file.path(), &records).unwrap();

    let adapter = NetworkRailTrainScheduleServiceAdapter::new(cache_file.path().to_path_buf());
    let date = chrono::NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();

    let passages = adapter.passages_on(date).await.unwrap();

    assert_that!(passages).has_length(1);
    assert_that!(passages[0].uid).is_equal_to("UID1".to_owned());
}
