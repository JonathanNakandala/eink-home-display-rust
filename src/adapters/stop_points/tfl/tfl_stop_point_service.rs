use anyhow::Context;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};

use crate::adapters::http;
use crate::domain::models::source_error::SourceError;
use crate::adapters::stop_points::tfl::response::{NearbyResponse, NearbyStop, SearchMatch, SearchResponse};
use crate::domain::models::location::Location;
use crate::domain::models::stop_point::{StopKind, StopPoint};
use crate::domain::services::stop_point_service::StopPointService;

pub const DEFAULT_HOST_URL: &str = "https://api.tfl.gov.uk";

#[derive(derive_new::new)]
pub struct TflStopPointServiceAdapter {
    host_url: String,
    app_key: Option<SecretString>,
    client: Client,
}

impl TflStopPointServiceAdapter {
    fn stop_type(kind: StopKind) -> &'static str {
        match kind {
            StopKind::Bus => "NaptanPublicBusCoachTram",
            StopKind::Underground => "NaptanMetroStation",
        }
    }

    fn mode(kind: StopKind) -> &'static str {
        match kind {
            StopKind::Bus => "bus",
            StopKind::Underground => "tube",
        }
    }

    /// GET `{host_url}/{path}`, adding the app key when configured and
    /// turning non-2xx statuses into errors.
    pub(super) async fn get(
        &self,
        path: &str,
        mut params: Vec<(&'static str, String)>,
    ) -> Result<reqwest::Response, SourceError> {
        if let Some(key) = &self.app_key {
            params.push(("app_key", key.expose_secret().to_owned()));
        }
        http::send(self.client.get(format!("{}/{}", self.host_url, path)).query(&params)).await
    }
}

impl StopPointService for TflStopPointServiceAdapter {
    async fn find_nearby(
        &self,
        location: Location,
        kind: StopKind,
        radius_metres: u32,
    ) -> anyhow::Result<Vec<StopPoint>> {
        let params = vec![
            ("lat", location.latitude.to_string()),
            ("lon", location.longitude.to_string()),
            ("stopTypes", Self::stop_type(kind).to_owned()),
            ("radius", radius_metres.to_string()),
        ];

        let response: NearbyResponse = self
            .get("StopPoint", params)
            .await
            .context("TfL nearby stop request failed")?
            .json()
            .await
            .context("Failed to parse TfL nearby stop response")?;

        let mut stops: Vec<StopPoint> = response.stop_points.into_iter().map(to_domain_nearby).collect();
        stops.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
        Ok(stops)
    }

    async fn search_by_name(&self, query: &str, kind: StopKind) -> anyhow::Result<Vec<StopPoint>> {
        let params = vec![
            ("query", query.to_owned()),
            ("modes", Self::mode(kind).to_owned()),
        ];

        let response: SearchResponse = self
            .get("StopPoint/Search", params)
            .await
            .context("TfL stop search request failed")?
            .json()
            .await
            .context("Failed to parse TfL stop search response")?;

        Ok(response.matches.into_iter().map(to_domain_match).collect())
    }
}

fn to_domain_nearby(stop: NearbyStop) -> StopPoint {
    StopPoint::new(
        stop.naptan_id,
        stop.common_name,
        stop.indicator.filter(|i| !i.is_empty()),
        stop.distance,
        stop.lines.into_iter().map(|l| l.name).collect(),
        stop.lat,
        stop.lon,
    )
}

fn to_domain_match(m: SearchMatch) -> StopPoint {
    StopPoint::new(m.id, m.name, None, None, Vec::new(), m.lat, m.lon)
}
