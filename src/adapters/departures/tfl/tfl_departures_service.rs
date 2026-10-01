use crate::adapters::stop_points::tfl::tfl_stop_point_service::TflStopPointServiceAdapter;
use crate::domain::models::arrival::Arrival;
use crate::domain::models::departures::DepartureService;
use crate::domain::services::arrivals_service::ArrivalsService;
use crate::domain::services::departures_service::DeparturesService;

/// Presents live TfL arrivals at one stop as a departures board.
#[derive(derive_new::new)]
pub struct TflDeparturesServiceAdapter {
    arrivals: TflStopPointServiceAdapter,
    stop_id: String,
}

impl DeparturesService for TflDeparturesServiceAdapter {
    async fn get_departures(&self, num_rows: u8) -> anyhow::Result<Vec<DepartureService>> {
        let arrivals = self.arrivals.get_arrivals(&self.stop_id).await?;
        Ok(arrivals
            .into_iter()
            .take(num_rows as usize)
            .map(to_domain_service)
            .collect())
    }
}

/// TfL only gives live predictions, so there is no scheduled time to compare
/// against: `time` is the countdown, and status/delay are left empty.
fn to_domain_service(arrival: Arrival) -> DepartureService {
    let time = match arrival.seconds_to_arrival {
        0..=29 => "due".to_owned(),
        s => format!("{} min", s / 60),
    };
    let destination = format!("{} {}", arrival.line, arrival.towards);
    DepartureService::new(time, destination, String::new(), String::new())
}
