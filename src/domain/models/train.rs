use chrono::NaiveDateTime;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, derive_new::new, Serialize)]
pub struct TrainPassage {
    pub uid: String,
    pub scheduled_at: NaiveDateTime,
}
