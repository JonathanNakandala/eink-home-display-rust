use chrono::{Duration, NaiveDateTime};
use serde::Serialize;

use crate::domain::models::air_quality::AirQuality;
use crate::domain::models::pollen::Pollen;

/// Serialised in lowercase to match the icon partial of the same name in the template.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WeatherCondition {
    Clear,
    Clouds,
    Drizzle,
    Rain,
    Thunderstorm,
    Snow,
}

#[derive(Debug, Clone, Serialize, derive_new::new, PartialEq)]
pub struct WeatherInformation {
    temperature: i8,
    min: i8,
    max: i8,
    condition: WeatherCondition,
    /// Left off the display when `None`, i.e. no precipitation is expected soon.
    #[new(default)]
    precipitation: Option<PrecipitationOutlook>,
    /// Left off the display when `None`, i.e. the provider has no air quality data.
    #[new(default)]
    air_quality: Option<AirQuality>,
    /// Left off the display when `None`, i.e. the day's UV is too low to matter.
    #[new(default)]
    uv_index: Option<UvIndex>,
    /// Only fetched when enabled in the config, and not on the display yet.
    #[new(default)]
    pollen: Option<Pollen>,
}

impl WeatherInformation {
    pub fn temperature(&self) -> i8 {
        self.temperature
    }

    pub fn with_precipitation(mut self, precipitation: Option<PrecipitationOutlook>) -> Self {
        self.precipitation = precipitation;
        self
    }

    pub fn with_pollen(mut self, pollen: Option<Pollen>) -> Self {
        self.pollen = pollen;
        self
    }

    pub fn with_uv_index(mut self, uv_index: Option<UvIndex>) -> Self {
        self.uv_index = uv_index;
        self
    }

    pub fn with_air_quality(mut self, air_quality: Option<AirQuality>) -> Self {
        self.air_quality = air_quality;
        self
    }
}

/// The day's highest UV index, with its WHO band. Shown even when Low, so a missing line
/// means the lookup failed, not that the UV is low.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UvIndex {
    value: u8,
    /// e.g. "High"
    band: &'static str,
}

impl UvIndex {
    pub fn new(max: f64) -> Self {
        let value = max.round().clamp(0.0, 99.0) as u8;
        let band = match value {
            0..=2 => "Low",
            3..=5 => "Moderate",
            6..=7 => "High",
            8..=10 => "Very high",
            _ => "Extreme",
        };
        Self { value, band }
    }
}

/// What is expected to fall, which sets the caption and the unit of the chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecipitationKind {
    Drizzle,
    FreezingDrizzle,
    Rain,
    FreezingRain,
    Snow,
    Thunderstorm,
}

impl PrecipitationKind {
    fn caption(self) -> &'static str {
        match self {
            PrecipitationKind::Drizzle => "UPCOMING DRIZZLE",
            PrecipitationKind::FreezingDrizzle => "UPCOMING FREEZING DRIZZLE",
            PrecipitationKind::Rain => "UPCOMING RAIN",
            PrecipitationKind::FreezingRain => "UPCOMING FREEZING RAIN",
            PrecipitationKind::Snow => "UPCOMING SNOW",
            PrecipitationKind::Thunderstorm => "UPCOMING THUNDERSTORM",
        }
    }

    fn unit(self) -> &'static str {
        match self {
            PrecipitationKind::Snow => "cm/h",
            _ => "mm/h",
        }
    }
}

/// One forecast interval, in order and of equal length.
#[derive(Debug, Clone, PartialEq, derive_new::new)]
pub struct PrecipitationSlot {
    /// Local time the interval starts.
    pub starts_at: NaiveDateTime,
    /// Water-equivalent precipitation, mm/h. Decides whether the slot is wet.
    pub precipitation: f64,
    /// Snowfall, cm/h. Charted instead of `precipitation` when the kind is snow.
    pub snowfall: f64,
    pub kind: PrecipitationKind,
}

/// Slots at or above this rate count as wet; below it is model noise.
const WET_MM_PER_HOUR: f64 = 0.1;
/// The chart is never scaled below this, so a drizzle doesn't fill it like a downpour.
const MIN_SCALE: f64 = 1.0;
const SLOT_MINUTES: i64 = 15;

// Chart geometry, in the units of the template's SVG viewBox.
const LEFT: f64 = 14.0;
const WIDTH: f64 = 144.0;
const HEIGHT: f64 = 46.0;
const TOP: f64 = 24.0;
const BASE: f64 = TOP + HEIGHT;
/// Closer than this and the peak label would sit on the start label, so it is raised above it.
const LABEL_CLASH: f64 = 42.0;
const LABEL_HALF_WIDTH: f64 = 26.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bar {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tick {
    x: f64,
    /// Ticks with a label are drawn longer.
    labelled: bool,
    label: String,
    /// SVG `text-anchor`, so the end labels stay inside the chart.
    anchor: &'static str,
}

/// A chart of the next couple of hours, ready for the template to draw.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrecipitationOutlook {
    /// e.g. "UPCOMING DRIZZLE"
    caption: String,
    bars: Vec<Bar>,
    ticks: Vec<Tick>,
    /// When the first wet slot starts, e.g. "11:45", or "Now" when it is already wet.
    start_label: String,
    start_x: f64,
    start_label_x: f64,
    /// Highest rate with its unit, e.g. "2 mm/h", shown above the tallest bar.
    peak_label: String,
    peak_x: f64,
    peak_y: f64,
}

impl PrecipitationOutlook {
    /// `None` when no slot is wet. `now` only decides between a start time and "Now".
    pub fn from_slots(slots: &[PrecipitationSlot], now: NaiveDateTime) -> Option<Self> {
        let is_wet = |slot: &PrecipitationSlot| slot.precipitation >= WET_MM_PER_HOUR;
        let first = slots.iter().position(is_wet)?;
        let kind = slots[first].kind;
        let amount = |slot: &PrecipitationSlot| match kind {
            PrecipitationKind::Snow => slot.snowfall,
            _ => slot.precipitation,
        };

        let count = slots.len() as f64;
        let slot_width = WIDTH / count;
        let peak = slots
            .iter()
            .filter(|slot| is_wet(slot))
            .map(amount)
            .fold(0.0, f64::max);
        let scale = peak.max(MIN_SCALE);

        let mut peak_index = first;
        let bars = slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| is_wet(slot))
            .map(|(i, slot)| {
                if amount(slot) > amount(&slots[peak_index]) {
                    peak_index = i;
                }
                let height = (amount(slot) / scale * HEIGHT).clamp(4.0, HEIGHT);
                Bar { x: LEFT + i as f64 * slot_width, y: BASE - height, width: slot_width, height }
            })
            .collect::<Vec<_>>();

        let start_x = LEFT + first as f64 * slot_width;
        let edge = LEFT + WIDTH - LABEL_HALF_WIDTH;
        let start_label = if slots[first].starts_at <= now {
            "Now".to_owned()
        } else {
            slots[first].starts_at.format("%H:%M").to_string()
        };

        let peak_x = (LEFT + (peak_index as f64 + 0.5) * slot_width)
            .clamp(LEFT + LABEL_HALF_WIDTH, edge);
        let peak_height = (amount(&slots[peak_index]) / scale * HEIGHT).clamp(4.0, HEIGHT);
        let clash = (peak_x - start_x).abs() < LABEL_CLASH;
        let peak_y = BASE - peak_height - if clash { 22.0 } else { 5.0 };

        let first_start = slots[0].starts_at;
        let ticks = [0, slots.len() / 2, slots.len()]
            .into_iter()
            .map(|i| Tick {
                x: LEFT + i as f64 * slot_width,
                labelled: true,
                label: (first_start + Duration::minutes(SLOT_MINUTES * i as i64))
                    .format("%H:%M")
                    .to_string(),
                anchor: match i {
                    0 => "start",
                    i if i == slots.len() => "end",
                    _ => "middle",
                },
            })
            .collect::<Vec<_>>();
        let ticks = (0..=slots.len())
            .map(|i| match ticks.iter().find(|t| t.x == LEFT + i as f64 * slot_width) {
                Some(tick) => tick.clone(),
                None => Tick {
                    x: LEFT + i as f64 * slot_width,
                    labelled: false,
                    label: String::new(),
                    anchor: "middle",
                },
            })
            .collect();

        Some(Self {
            caption: kind.caption().to_owned(),
            bars,
            ticks,
            start_label,
            start_x,
            start_label_x: start_x.clamp(LEFT + 14.0, LEFT + WIDTH - 17.0),
            peak_label: format!("{} {}", format_rate(peak), kind.unit()),
            peak_x,
            peak_y,
        })
    }
}

/// "0.3", "2" or "12": one decimal while it matters.
fn format_rate(rate: f64) -> String {
    if rate >= 10.0 {
        format!("{rate:.0}")
    } else {
        format!("{rate:.1}").trim_end_matches(".0").to_owned()
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn at(hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 1).unwrap().and_hms_opt(hour, minute, 0).unwrap()
    }

    /// Eight slots from 11:15, with the given mm/h for each.
    fn slots(rates: [f64; 8], kind: PrecipitationKind) -> Vec<PrecipitationSlot> {
        rates
            .iter()
            .enumerate()
            .map(|(i, rate)| {
                PrecipitationSlot::new(at(11, 15) + Duration::minutes(15 * i as i64), *rate, *rate, kind)
            })
            .collect()
    }

    #[test]
    fn uv_is_banded_including_when_low() {
        assert_eq!(UvIndex::new(0.0), UvIndex { value: 0, band: "Low" });
        assert_eq!(UvIndex::new(2.4), UvIndex { value: 2, band: "Low" });
        assert_eq!(UvIndex::new(2.6), UvIndex { value: 3, band: "Moderate" });
        assert_eq!(UvIndex::new(6.0), UvIndex { value: 6, band: "High" });
        assert_eq!(UvIndex::new(8.4), UvIndex { value: 8, band: "Very high" });
        assert_eq!(UvIndex::new(11.2), UvIndex { value: 11, band: "Extreme" });
    }

    #[test]
    fn nothing_when_it_stays_dry() {
        let dry = slots([0.0, 0.0, 0.0, 0.05, 0.0, 0.0, 0.0, 0.0], PrecipitationKind::Rain);
        assert_eq!(PrecipitationOutlook::from_slots(&dry, at(11, 11)), None);
    }

    #[test]
    fn describes_when_it_starts_and_how_much_at_most() {
        let wet = slots([0.0, 0.0, 0.0, 0.4, 1.2, 2.0, 1.0, 0.3], PrecipitationKind::Drizzle);
        let outlook = PrecipitationOutlook::from_slots(&wet, at(11, 11)).unwrap();
        assert_eq!(outlook.caption, "UPCOMING DRIZZLE");
        assert_eq!(outlook.start_label, "12:00");
        assert_eq!(outlook.peak_label, "2 mm/h");
        assert_eq!(outlook.bars.len(), 5);
        assert_eq!(
            outlook.ticks.iter().filter(|t| t.labelled).map(|t| t.label.as_str()).collect::<Vec<_>>(),
            ["11:15", "12:15", "13:15"]
        );
    }

    #[test]
    fn says_now_when_already_wet() {
        let wet = slots([0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], PrecipitationKind::Rain);
        let outlook = PrecipitationOutlook::from_slots(&wet, at(11, 20)).unwrap();
        assert_eq!(outlook.start_label, "Now");
    }

    #[test]
    fn light_rain_is_drawn_small_not_scaled_to_fill_the_chart() {
        let light = slots([0.0, 0.0, 0.0, 0.1, 0.2, 0.3, 0.2, 0.1], PrecipitationKind::Rain);
        let outlook = PrecipitationOutlook::from_slots(&light, at(11, 11)).unwrap();
        let tallest = outlook.bars.iter().map(|bar| bar.height).fold(0.0, f64::max);
        assert!(tallest < HEIGHT / 2.0, "tallest bar was {tallest}");
        assert_eq!(outlook.peak_label, "0.3 mm/h");
    }

    #[test]
    fn snow_is_charted_in_snowfall_units() {
        let mut snow = slots([0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0], PrecipitationKind::Snow);
        snow[2].snowfall = 1.4;
        let outlook = PrecipitationOutlook::from_slots(&snow, at(11, 11)).unwrap();
        assert_eq!(outlook.caption, "UPCOMING SNOW");
        assert_eq!(outlook.peak_label, "1.4 cm/h");
    }

    #[test]
    fn peak_label_is_raised_when_it_would_sit_on_the_start_label() {
        let close = slots([0.0, 0.0, 0.0, 2.0, 0.5, 0.0, 0.0, 0.0], PrecipitationKind::Rain);
        let far = slots([0.0, 0.0, 0.0, 0.5, 1.0, 2.0, 0.0, 0.0], PrecipitationKind::Rain);
        let close = PrecipitationOutlook::from_slots(&close, at(11, 11)).unwrap();
        let far = PrecipitationOutlook::from_slots(&far, at(11, 11)).unwrap();
        assert!(close.peak_y < far.peak_y);
    }
}
