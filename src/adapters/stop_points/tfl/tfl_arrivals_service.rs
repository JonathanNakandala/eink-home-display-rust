use crate::adapters::http;
use crate::adapters::stop_points::tfl::response::Prediction;
use crate::adapters::stop_points::tfl::tfl_stop_point_service::TflStopPointServiceAdapter;
use crate::domain::models::arrival::Arrival;
use crate::domain::models::source_error::SourceError;
use crate::domain::services::arrivals_service::ArrivalsService;

impl ArrivalsService for TflStopPointServiceAdapter {
    async fn get_arrivals(&self, stop_id: &str) -> Result<Vec<Arrival>, SourceError> {
        let response = self
            .get(&format!("StopPoint/{stop_id}/Arrivals"), Vec::new())
            .await?;
        let predictions: Vec<Prediction> = http::json(response, "TfL arrivals").await?;

        let mut arrivals: Vec<Arrival> = predictions.into_iter().map(to_domain).collect();
        arrivals.sort_by_key(|a| a.seconds_to_arrival);
        Ok(arrivals)
    }
}

fn to_domain(p: Prediction) -> Arrival {
    Arrival::new(
        p.line_name,
        p.destination_name,
        p.towards,
        p.platform_name,
        p.current_location,
        p.time_to_station,
        p.station_name,
    )
}
