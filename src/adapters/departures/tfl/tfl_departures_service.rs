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
    /// Minutes to get to the stop; arrivals sooner than this are left off.
    travel_minutes: u16,
}

impl DeparturesService for TflDeparturesServiceAdapter {
    async fn get_departures(&self, num_rows: u8) -> anyhow::Result<Vec<DepartureService>> {
        let arrivals = self.arrivals.get_arrivals(&self.stop_id).await?;
        Ok(catchable(arrivals, self.travel_minutes)
            .take(num_rows as usize)
            .map(to_domain_service)
            .collect())
    }
}

/// Arrivals we can still reach the stop in time for, in their original order.
fn catchable(
    arrivals: Vec<Arrival>,
    travel_minutes: u16,
) -> impl Iterator<Item = Arrival> {
    let travel_seconds = u32::from(travel_minutes) * 60;
    arrivals
        .into_iter()
        .filter(move |a| a.seconds_to_arrival >= travel_seconds)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn arrival_in(seconds: u32) -> Arrival {
        Arrival::new(
            "Piccadilly".into(),
            "Cockfosters".into(),
            "Cockfosters".into(),
            "1".into(),
            String::new(),
            seconds,
        )
    }

    #[test]
    fn leaves_off_arrivals_sooner_than_the_travel_time() {
        let arrivals = vec![arrival_in(60), arrival_in(14 * 60 + 59), arrival_in(15 * 60), arrival_in(20 * 60)];

        let times: Vec<u32> = catchable(arrivals, 15).map(|a| a.seconds_to_arrival).collect();

        assert_eq!(times, vec![15 * 60, 20 * 60]);
    }

    #[test]
    fn keeps_everything_without_a_travel_time() {
        assert_eq!(catchable(vec![arrival_in(0), arrival_in(60)], 0).count(), 2);
    }
}
