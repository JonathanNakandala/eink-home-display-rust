use serde::Deserialize;

/// A single NDJSON line from the CIF SCHEDULE feed carries exactly one of
/// these record types under its own key; unrelated record types already
/// present in the feed are ignored by serde's default "unknown fields" rules.
#[derive(Debug, Deserialize)]
pub struct RecordEnvelope {
    #[serde(rename = "JsonScheduleV1")]
    pub schedule: Option<JsonScheduleV1>,
    #[serde(rename = "TiplocV1")]
    pub tiploc: Option<TiplocV1>,
}

#[derive(Debug, Deserialize)]
pub struct JsonScheduleV1 {
    pub transaction_type: Option<String>,
    #[serde(rename = "CIF_train_uid")]
    pub train_uid: String,
    pub schedule_start_date: String,
    pub schedule_end_date: String,
    pub schedule_days_runs: String,
    #[serde(rename = "CIF_stp_indicator")]
    pub stp_indicator: String,
    pub train_status: Option<String>,
    pub schedule_segment: Option<ScheduleSegment>,
}

#[derive(Debug, Deserialize)]
pub struct ScheduleSegment {
    pub schedule_location: Option<Vec<ScheduleLocation>>,
}

#[derive(Debug, Deserialize)]
pub struct ScheduleLocation {
    pub tiploc_code: Option<String>,
    pub departure: Option<String>,
    pub arrival: Option<String>,
    pub pass: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TiplocV1 {
    pub tiploc_code: String,
    pub tps_description: Option<String>,
}
