use anyhow::{bail, Context};

use crate::adapters::departures::{DeparturesServiceImpl, OpenLdbwsDeparturesServiceAdapter, TflDeparturesServiceAdapter};
use crate::adapters::stop_points::tfl::tfl_stop_point_service::TflStopPointServiceAdapter;
use crate::application::DepartureBoard;
use crate::config::departures::{DepartureBoardConfig, DepartureSource, ProvidersConfig, DEFAULT_TFL_HOST_URL};

/// Builds one board per enabled `[[departures]]` entry, in config order.
pub fn setup_departure_boards(
    boards: &[DepartureBoardConfig],
    providers: &ProvidersConfig,
) -> anyhow::Result<Vec<DepartureBoard<DeparturesServiceImpl>>> {
    boards
        .iter()
        .filter(|board| board.enabled)
        .map(|board| {
            let service = setup_service(board, providers)
                .with_context(|| format!("Invalid departures board \"{}\"", board.name))?;
            Ok(DepartureBoard::new(board.name.clone(), board.rows, service))
        })
        .collect()
}

fn setup_service(
    board: &DepartureBoardConfig,
    providers: &ProvidersConfig,
) -> anyhow::Result<DeparturesServiceImpl> {
    match &board.source {
        DepartureSource::OpenLdbws { from, to } => {
            let Some(config) = &providers.open_ldbws else {
                bail!("provider OpenLdbws requires a [providers.open_ldbws] section");
            };
            Ok(DeparturesServiceImpl::OpenLdbws(
                OpenLdbwsDeparturesServiceAdapter::new(
                    config.host_url.clone(),
                    config.api_key.clone(),
                    from.clone(),
                    to.clone(),
                    board.travel_minutes,
                    crate::adapters::http::client(),
                ),
            ))
        }
        DepartureSource::Tfl { stop_id } => {
            // The TfL section is optional: anonymous access to the default host works.
            let (host_url, app_key) = match &providers.tfl {
                Some(config) => (config.host_url.clone(), config.app_key.clone()),
                None => (DEFAULT_TFL_HOST_URL.to_owned(), None),
            };
            Ok(DeparturesServiceImpl::Tfl(TflDeparturesServiceAdapter::new(
                TflStopPointServiceAdapter::new(host_url, app_key, crate::adapters::http::client()),
                stop_id.clone(),
                board.travel_minutes,
            )))
        }
    }
}
