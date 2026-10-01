use anyhow::Context;
use reqwest::Client;

use crate::adapters::departures::open_ldbws::response::{Service, StationBoard};
use crate::domain::models::departures::DepartureService;
use crate::domain::services::departures_service::DeparturesService;

/// `host_url` is the Rail Data Marketplace product's base path up to and
/// including the operation name, e.g.
/// `https://api1.raildata.org.uk/1010-live-arrival-and-departure-boards-arr-and-dep1_1/LDBWS/api/20220120/GetArrDepBoardWithDetails`
/// — the `from` station CRS code is appended as the final path segment, and
/// `to` is used as the destination filter.
#[derive(derive_new::new)]
pub struct OpenLdbwsDeparturesServiceAdapter {
    host_url: String,
    api_key: String,
    from: String,
    to: String,
    client: Client,
}

impl DeparturesService for OpenLdbwsDeparturesServiceAdapter {
    async fn get_departures(&self, num_rows: u8) -> anyhow::Result<Vec<DepartureService>> {
        let url = format!("{}/{}", self.host_url, self.from);

        let response = self
            .client
            .get(&url)
            .header("x-apikey", &self.api_key)
            .query(&[
                ("numRows", num_rows.to_string()),
                ("filterCrs", self.to.clone()),
                ("filterType", "to".to_owned()),
            ])
            .send()
            .await?
            .error_for_status()
            .context("National Rail departure board request failed")?;

        let station_board: StationBoard = response
            .json()
            .await
            .context("Failed to parse departure board response")?;

        let services = station_board.train_services.unwrap_or_default();

        Ok(services.into_iter().filter_map(to_domain_service).collect())
    }
}

/// Entries without a scheduled departure time are arrivals-only (the board
/// covers both), and aren't relevant to a departures listing.
fn to_domain_service(service: Service) -> Option<DepartureService> {
    let std = service.std?;
    let destination = service
        .destination
        .first()
        .map(|l| l.location_name.clone())
        .unwrap_or_default();

    let (status, delay) = match service.etd.as_deref() {
        Some("On time") | None => ("On time".to_owned(), String::new()),
        Some("Cancelled") => ("Cancelled".to_owned(), String::new()),
        Some(estimated) => ("Delayed".to_owned(), estimated.to_owned()),
    };

    Some(DepartureService::new(std, destination, status, delay))
}
