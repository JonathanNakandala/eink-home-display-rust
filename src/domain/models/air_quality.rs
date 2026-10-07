use serde::Serialize;

/// The European Air Quality Index bands, which Open-Meteo scales to 0-100 and beyond.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirQualityBand {
    Good,
    Fair,
    Moderate,
    Poor,
    VeryPoor,
    ExtremelyPoor,
}

impl AirQualityBand {
    pub fn from_score(score: u16) -> Self {
        match score {
            0..=19 => AirQualityBand::Good,
            20..=39 => AirQualityBand::Fair,
            40..=59 => AirQualityBand::Moderate,
            60..=79 => AirQualityBand::Poor,
            80..=100 => AirQualityBand::VeryPoor,
            _ => AirQualityBand::ExtremelyPoor,
        }
    }

    fn label(self) -> &'static str {
        match self {
            AirQualityBand::Good => "Good",
            AirQualityBand::Fair => "Fair",
            AirQualityBand::Moderate => "Moderate",
            AirQualityBand::Poor => "Poor",
            AirQualityBand::VeryPoor => "Very poor",
            AirQualityBand::ExtremelyPoor => "Extremely poor",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AirQualityReading {
    /// e.g. "PM10"
    label: String,
    score: u16,
    /// e.g. "Poor"
    band: &'static str,
    /// Drawn lighter than the rest, so what needs attention stands out.
    good: bool,
}

impl AirQualityReading {
    fn new(label: &str, score: f64) -> Self {
        let score = score.round().max(0.0) as u16;
        let band = AirQualityBand::from_score(score);
        Self {
            label: label.to_owned(),
            score,
            band: band.label(),
            good: band == AirQualityBand::Good,
        }
    }
}

/// The overall index and each pollutant's own, worst first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AirQuality {
    overall: AirQualityReading,
    pollutants: Vec<AirQualityReading>,
}

impl AirQuality {
    /// `overall` falls back to the worst pollutant, which is how the index is defined.
    /// `None` when there is nothing to show.
    pub fn new(overall: Option<f64>, pollutants: &[(&str, Option<f64>)]) -> Option<Self> {
        let mut pollutants: Vec<_> = pollutants
            .iter()
            .filter_map(|(label, score)| Some(AirQualityReading::new(label, (*score)?)))
            .collect();
        // Stable, so equal scores keep the order they were given in.
        pollutants.sort_by_key(|pollutant| std::cmp::Reverse(pollutant.score));
        let overall = match overall {
            Some(score) => AirQualityReading::new("Overall", score),
            None => AirQualityReading::new("Overall", f64::from(pollutants.first()?.score)),
        };
        Some(Self {
            overall,
            pollutants,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_follow_the_european_thresholds() {
        let band = |score| AirQualityBand::from_score(score).label();
        assert_eq!(
            [0, 19, 20, 39, 40, 59, 60, 79, 80, 100, 101].map(band),
            [
                "Good",
                "Good",
                "Fair",
                "Fair",
                "Moderate",
                "Moderate",
                "Poor",
                "Poor",
                "Very poor",
                "Very poor",
                "Extremely poor"
            ]
        );
    }

    #[test]
    fn lists_pollutants_worst_first_and_marks_good_ones() {
        let air = AirQuality::new(
            Some(62.0),
            &[
                ("PM2.5", Some(31.0)),
                ("PM10", Some(62.0)),
                ("Ozone", Some(18.0)),
                ("SO₂", None),
            ],
        )
        .unwrap();
        assert_eq!(air.overall.band, "Poor");
        let labels: Vec<_> = air.pollutants.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["PM10", "PM2.5", "Ozone"]);
        assert_eq!(
            air.pollutants.iter().map(|p| p.good).collect::<Vec<_>>(),
            [false, false, true]
        );
    }

    #[test]
    fn overall_falls_back_to_the_worst_pollutant() {
        let air = AirQuality::new(None, &[("NO₂", Some(27.0)), ("PM10", Some(45.0))]).unwrap();
        assert_eq!(air.overall.score, 45);
        assert_eq!(air.overall.band, "Moderate");
    }

    #[test]
    fn nothing_to_show_without_any_values() {
        assert_eq!(AirQuality::new(None, &[("PM10", None)]), None);
    }
}
