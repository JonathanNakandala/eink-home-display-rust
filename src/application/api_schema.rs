//! How the domain types that appear in the status JSON are described in the API description.
//!
//! The domain carries no schema derives (see `tests/architecture.rs`), so what it serialises is described
//! here instead, in types that are never sent: they exist for `#[schema(value_type = ...)]` to point at. A
//! description that is written separately can drift, so each one is built from the real thing by a `From`
//! with an exhaustive `match`: a variant added to the domain stops this compiling until it is described, and
//! a test checks the two serialise to the same JSON.

use serde::Serialize;
use utoipa::ToSchema;

use crate::domain::models::display::ImageFormat;
use crate::domain::models::render_report::{SourceReport, SourceState};

/// A file format the server can send an image in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = ImageFormat)]
pub enum ImageFormatSchema {
    Bmp,
    Png,
    Qoi,
}

impl From<ImageFormat> for ImageFormatSchema {
    fn from(format: ImageFormat) -> Self {
        match format {
            ImageFormat::Bmp => Self::Bmp,
            ImageFormat::Png => Self::Png,
            ImageFormat::Qoi => Self::Qoi,
        }
    }
}

/// How a data source stood at the last render. `state` says which, and the fields beside it depend on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
#[schema(as = SourceState)]
pub enum SourceStateSchema {
    /// Answered just now.
    Fresh,
    /// Failed, and shown from an earlier answer this many seconds old.
    Stale { age_seconds: u64 },
    /// Failed, with nothing recent enough to show. `reason` is a few words, such as "key rejected".
    Unavailable { reason: String },
}

impl From<&SourceState> for SourceStateSchema {
    fn from(state: &SourceState) -> Self {
        match state {
            SourceState::Fresh => Self::Fresh,
            SourceState::Stale { age_seconds } => Self::Stale {
                age_seconds: *age_seconds,
            },
            SourceState::Unavailable { reason } => Self::Unavailable {
                reason: reason.clone(),
            },
        }
    }
}

/// One data source (weather, a departure board) and how it was on the last render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[schema(as = SourceReport)]
pub struct SourceReportSchema {
    pub name: String,
    #[serde(flatten)]
    pub state: SourceStateSchema,
}

impl From<&SourceReport> for SourceReportSchema {
    fn from(report: &SourceReport) -> Self {
        Self {
            name: report.name.clone(),
            state: (&report.state).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_description_serialises_exactly_as_the_domain_type_does() {
        for format in ImageFormat::ALL {
            assert_eq!(
                serde_json::to_value(format).unwrap(),
                serde_json::to_value(ImageFormatSchema::from(format)).unwrap()
            );
        }
        for state in [
            SourceState::Fresh,
            SourceState::Stale { age_seconds: 90 },
            SourceState::Unavailable {
                reason: "key rejected".to_owned(),
            },
        ] {
            let report = SourceReport {
                name: "weather".to_owned(),
                state,
            };
            assert_eq!(
                serde_json::to_value(&report).unwrap(),
                serde_json::to_value(SourceReportSchema::from(&report)).unwrap(),
                "{report:?}"
            );
        }
    }
}
