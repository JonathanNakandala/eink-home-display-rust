use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, derive_new::new, Serialize)]
pub struct DepartureService {
    pub time: String,
    pub destination: String,
    pub status: String,
    pub delay: String,
}
