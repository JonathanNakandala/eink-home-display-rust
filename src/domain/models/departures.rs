use serde::Serialize;

#[derive(Debug, Clone, derive_new::new)]
pub struct StationPair {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, derive_new::new, Serialize)]
pub struct DepartureService {
    pub time: String,
    pub destination: String,
    pub status: String,
    pub delay: String,
}
