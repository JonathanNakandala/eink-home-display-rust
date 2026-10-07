use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use reqwest::Client;
use reqwest::header::HeaderValue;
use secrecy::{ExposeSecret, SecretString};

use crate::adapters::departures::open_ldbws::response::{Service, StationBoard};
use crate::adapters::http;
use crate::domain::models::departures::{
    DepartureService, DepartureStatus, Departures, clock_time_near, countdown, seconds_to,
};
use crate::domain::models::source_error::SourceError;
use crate::domain::services::departures_service::DeparturesService;

/// `host_url` is the Rail Data Marketplace product's base path up to and
/// including the operation name, e.g.
/// `https://api1.raildata.org.uk/1010-live-arrival-and-departure-boards-arr-and-dep1_1/LDBWS/api/20220120/GetArrDepBoardWithDetails`
/// — the `from` station CRS code is appended as the final path segment, and
/// `to` is used as the destination filter.
#[derive(derive_new::new)]
pub struct OpenLdbwsDeparturesServiceAdapter {
    host_url: String,
    api_key: SecretString,
    from: String,
    to: String,
    /// Minutes to get to the station; the board is requested as it will be then.
    travel_minutes: u16,
    client: Client,
}

/// The largest `timeOffset` the API accepts, in minutes.
const MAX_TIME_OFFSET: u16 = 119;

impl DeparturesService for OpenLdbwsDeparturesServiceAdapter {
    async fn get_departures(
        &self,
        num_rows: u8,
        now: DateTime<Tz>,
    ) -> Result<Departures, SourceError> {
        let url = format!("{}/{}", self.host_url, self.from);

        let mut query = vec![
            ("numRows", num_rows.to_string()),
            ("filterCrs", self.to.clone()),
            ("filterType", "to".to_owned()),
        ];
        if self.travel_minutes > 0 {
            // Ask for the board as of when we could get there, so trains we'd miss are never listed.
            query.push((
                "timeOffset",
                self.travel_minutes.min(MAX_TIME_OFFSET).to_string(),
            ));
        }

        // Marked sensitive, so reqwest and hyper leave it out of any debug output of the request.
        let mut key = HeaderValue::from_str(self.api_key.expose_secret()).map_err(|_| {
            SourceError::bad_response("the API key has characters a header can't hold")
        })?;
        key.set_sensitive(true);
        let response =
            http::send(self.client.get(&url).header("x-apikey", key).query(&query)).await?;
        let station_board: StationBoard = http::json(response, "departure board").await?;

        let services = station_board.train_services.unwrap_or_default();

        Ok(Departures::new(
            station_board.location_name.unwrap_or_default(),
            services
                .into_iter()
                .filter_map(|service| to_domain_service(service, now))
                .collect(),
        ))
    }
}

/// Rail Data reports every time on the UK clock, with no date or offset. The display may be in another zone, so a
/// time is read as UK time and then shown on the display's clock: reading it as the display's own gives a wrong
/// countdown, and shows the wrong time on the board.
const PROVIDER_ZONE: Tz = Tz::Europe__London;

/// A time the board gave, as the moment it means (None if it isn't a clock time) and as it reads on the display.
fn on_display_clock(board_time: &str, now: DateTime<Tz>) -> (Option<DateTime<Utc>>, String) {
    match clock_time_near(now, board_time, PROVIDER_ZONE) {
        Some(at) => (Some(at), at.with_timezone(&now.timezone()).format("%H:%M").to_string()),
        None => (None, board_time.to_owned()),
    }
}

/// Entries without a scheduled departure time are arrivals-only (the board
/// covers both), and aren't relevant to a departures listing.
fn to_domain_service(service: Service, now: DateTime<Tz>) -> Option<DepartureService> {
    let destination = service
        .destination
        .first()
        .map(|l| l.location_name.clone())
        .unwrap_or_default();
    let (scheduled, std) = on_display_clock(&service.std?, now);

    // The status, what to show as expected, and the moment to count down to
    // (none when there is no reliable one).
    let (status, expected, leaves_at) = match service.etd.as_deref() {
        Some("On time") | None => (DepartureStatus::OnTime, String::new(), scheduled),
        Some("Cancelled") => (DepartureStatus::Cancelled, String::new(), None),
        // Usually an estimated time like "18:27"; otherwise the board only says "Delayed".
        Some(estimated) if estimated.contains(':') => {
            let (at, shown) = on_display_clock(estimated, now);
            (DepartureStatus::Delayed, shown, at)
        }
        Some(_) => (DepartureStatus::Delayed, "late".to_owned(), None),
    };
    let countdown = leaves_at
        .map(|at| countdown(seconds_to(now, at)))
        .unwrap_or_default();

    Some(DepartureService::new(
        std,
        destination,
        status,
        expected,
        countdown,
    ))
}
