use std::collections::HashSet;

use chrono::{DateTime, Duration, Local};

use crate::adapters::stop_points::tfl::tfl_stop_point_service::TflStopPointServiceAdapter;
use crate::domain::models::arrival::Arrival;
use crate::domain::models::departures::{countdown, DepartureService, DepartureStatus, Departures};
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
    async fn get_departures(
        &self,
        num_rows: u8,
        now: DateTime<Local>,
    ) -> anyhow::Result<Departures> {
        let arrivals = self.arrivals.get_arrivals(&self.stop_id).await?;
        let show_line = serves_several_lines(&arrivals);
        let station = arrivals
            .first()
            .map(|a| station_name(&a.station_name))
            .unwrap_or_default();
        let services = catchable(arrivals, self.travel_minutes)
            .take(num_rows as usize)
            .map(|arrival| to_domain_service(arrival, show_line, now))
            .collect();
        Ok(Departures::new(station, services))
    }
}

/// TfL names stops like "Turnpike Lane Underground Station"; the mode is just noise here.
fn station_name(name: &str) -> String {
    const SUFFIXES: [&str; 4] = [" Underground Station", " Rail Station", " DLR Station", " Station"];
    SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name)
        .to_owned()
}

/// Whether the stop's arrivals come from more than one line. Only then is it worth
/// saying which line each is on: a station with one line (or a stop with one bus
/// route) gains nothing, while an interchange needs it.
fn serves_several_lines(arrivals: &[Arrival]) -> bool {
    arrivals.iter().map(|a| a.line.as_str()).collect::<HashSet<_>>().len() > 1
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
/// against: `time` is when it is predicted to arrive, and the status is `Live`.
fn to_domain_service(arrival: Arrival, show_line: bool, now: DateTime<Local>) -> DepartureService {
    let seconds = i64::from(arrival.seconds_to_arrival);
    let time = (now + Duration::seconds(seconds)).format("%H:%M").to_string();
    let destination = if show_line {
        format!("{} {}", arrival.line, arrival.towards)
    } else {
        arrival.towards
    };
    DepartureService::new(time, destination, DepartureStatus::Live, String::new(), countdown(seconds))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn arrival_on(line: &str, seconds: u32) -> Arrival {
        Arrival::new(
            line.into(),
            "Cockfosters".into(),
            "Cockfosters".into(),
            "1".into(),
            String::new(),
            seconds,
            "Turnpike Lane Underground Station".into(),
        )
    }

    fn arrival_in(seconds: u32) -> Arrival {
        arrival_on("Piccadilly", seconds)
    }

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2024, 1, 10, 13, 6, 0).unwrap()
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

    #[test]
    fn gives_the_predicted_time_and_the_countdown() {
        let service = to_domain_service(arrival_in(6 * 60 + 10), false, now());

        assert_eq!(
            service,
            DepartureService::new(
                "13:12".into(),
                "Cockfosters".into(),
                DepartureStatus::Live,
                String::new(),
                "6 min".into()
            )
        );
    }

    #[test]
    fn drops_the_mode_from_the_station_name() {
        assert_eq!(station_name("Turnpike Lane Underground Station"), "Turnpike Lane");
        assert_eq!(station_name("Hornsey Rail Station"), "Hornsey");
        assert_eq!(station_name("Turnpike Lane Station"), "Turnpike Lane");
        assert_eq!(station_name("Turnpike Lane"), "Turnpike Lane");
    }

    #[test]
    fn names_the_line_only_where_the_stop_has_several() {
        assert!(!serves_several_lines(&[arrival_on("Piccadilly", 60), arrival_on("Piccadilly", 120)]));
        assert!(serves_several_lines(&[arrival_on("Piccadilly", 60), arrival_on("Victoria", 120)]));
        assert_eq!(to_domain_service(arrival_in(60), true, now()).destination, "Piccadilly Cockfosters");
    }
}
