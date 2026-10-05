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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RenderReport {
    pub sources: Vec<SourceReport>,
}

impl RenderReport {
    /// Whether any source was shown from old data or left off.
    pub fn is_degraded(&self) -> bool {
        self.sources.iter().any(|source| source.state != SourceState::Fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(states: Vec<SourceState>) -> RenderReport {
        RenderReport {
            sources: states
                .into_iter()
                .enumerate()
                .map(|(i, state)| SourceReport { name: format!("source {i}"), state })
                .collect(),
        }
    }

    #[test]
    fn degraded_means_any_source_that_is_not_fresh() {
        assert!(!RenderReport::default().is_degraded());
        assert!(!report(vec![SourceState::Fresh, SourceState::Fresh]).is_degraded());
        assert!(report(vec![SourceState::Fresh, SourceState::Stale { age_seconds: 60 }]).is_degraded());
        assert!(report(vec![SourceState::Unavailable { reason: "timed out".into() }]).is_degraded());
    }

    #[test]
    fn serialises_flat_with_a_state_tag() {
        let source = SourceReport { name: "weather".into(), state: SourceState::Unavailable { reason: "key rejected".into() } };
        assert_eq!(
            serde_json::to_value(&source).unwrap(),
            serde_json::json!({"name": "weather", "state": "unavailable", "reason": "key rejected"})
        );
        let stale = SourceReport { name: "NORTHBOUND".into(), state: SourceState::Stale { age_seconds: 480 } };
        assert_eq!(
            serde_json::to_value(&stale).unwrap(),
            serde_json::json!({"name": "NORTHBOUND", "state": "stale", "age_seconds": 480})
        );
    }
}
