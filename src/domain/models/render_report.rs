//! What a render found out about each data source, for the status page: not what was drawn, but
//! how healthy the sources were while drawing it.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SourceState {
    /// Answered just now.
    Fresh,
    /// Failed, and shown from an earlier answer this many seconds old.
    Stale { age_seconds: u64 },
    /// Failed, with nothing recent enough to show. `reason` is a few words, e.g. "key rejected".
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceReport {
    pub name: String,
    #[serde(flatten)]
    pub state: SourceState,
}

/// How drawing one layout went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LayoutState {
    /// Drawn and published just now.
    Drawn,
    /// Not drawn this time (it failed, or took too long), so its last image stays in place. `reason` is a few words.
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LayoutReport {
    pub name: String,
    #[serde(flatten)]
    pub state: LayoutState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RenderReport {
    pub sources: Vec<SourceReport>,
    /// The layouts drawn on this run, the default first. Only the default failing fails the run, so any listed here
    /// as failed is one whose last image is still being served.
    pub layouts: Vec<LayoutReport>,
}

impl RenderReport {
    /// Whether any source was shown from old data or left off, or any layout could not be drawn.
    pub fn is_degraded(&self) -> bool {
        self.sources
            .iter()
            .any(|source| source.state != SourceState::Fresh)
            || self
                .layouts
                .iter()
                .any(|layout| layout.state != LayoutState::Drawn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(states: Vec<SourceState>) -> RenderReport {
        RenderReport {
            layouts: vec![],
            sources: states
                .into_iter()
                .enumerate()
                .map(|(i, state)| SourceReport {
                    name: format!("source {i}"),
                    state,
                })
                .collect(),
        }
    }

    #[test]
    fn degraded_means_any_source_that_is_not_fresh() {
        assert!(!RenderReport::default().is_degraded());
        assert!(!report(vec![SourceState::Fresh, SourceState::Fresh]).is_degraded());
        assert!(
            report(vec![
                SourceState::Fresh,
                SourceState::Stale { age_seconds: 60 }
            ])
            .is_degraded()
        );
        assert!(
            report(vec![SourceState::Unavailable {
                reason: "timed out".into()
            }])
            .is_degraded()
        );
    }

    #[test]
    fn a_layout_that_could_not_be_drawn_is_degraded() {
        let drawn = |name: &str| LayoutReport {
            name: name.to_owned(),
            state: LayoutState::Drawn,
        };
        let mut report = RenderReport {
            sources: vec![],
            layouts: vec![drawn("dashboard"), drawn("departures")],
        };
        assert!(!report.is_degraded());

        report.layouts[1].state = LayoutState::Failed {
            reason: "timed out".to_owned(),
        };

        assert!(report.is_degraded());
        assert_eq!(
            serde_json::to_value(&report.layouts[1]).unwrap(),
            serde_json::json!({"name": "departures", "state": "failed", "reason": "timed out"})
        );
    }

    #[test]
    fn serialises_flat_with_a_state_tag() {
        let source = SourceReport {
            name: "weather".into(),
            state: SourceState::Unavailable {
                reason: "key rejected".into(),
            },
        };
        assert_eq!(
            serde_json::to_value(&source).unwrap(),
            serde_json::json!({"name": "weather", "state": "unavailable", "reason": "key rejected"})
        );
        let stale = SourceReport {
            name: "NORTHBOUND".into(),
            state: SourceState::Stale { age_seconds: 480 },
        };
        assert_eq!(
            serde_json::to_value(&stale).unwrap(),
            serde_json::json!({"name": "NORTHBOUND", "state": "stale", "age_seconds": 480})
        );
    }
}
