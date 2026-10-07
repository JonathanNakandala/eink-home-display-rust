use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PollenType {
    Alder,
    Birch,
    Grass,
    Mugwort,
    Olive,
    Ragweed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PollenReading {
    #[serde(rename = "type")]
    kind: PollenType,
    /// Grains per cubic metre. Zero out of season.
    grains: f64,
}

impl PollenReading {
    pub fn kind(&self) -> PollenType {
        self.kind
    }

    pub fn grains(&self) -> f64 {
        self.grains
    }
}

/// Current pollen counts, in the order given. No severity bands yet: they differ per plant
/// and are left until the display needs them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pollen {
    readings: Vec<PollenReading>,
}

impl Pollen {
    /// Types the provider has no value for are left out; `None` when none have one.
    pub fn new(readings: &[(PollenType, Option<f64>)]) -> Option<Self> {
        let readings: Vec<_> = readings
            .iter()
            .filter_map(|(kind, grains)| {
                Some(PollenReading {
                    kind: *kind,
                    grains: (*grains)?.max(0.0),
                })
            })
            .collect();
        (!readings.is_empty()).then_some(Self { readings })
    }

    pub fn readings(&self) -> &[PollenReading] {
        &self.readings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_values_the_provider_has_in_order() {
        let pollen = Pollen::new(&[
            (PollenType::Grass, Some(85.0)),
            (PollenType::Olive, None),
            (PollenType::Birch, Some(0.0)),
        ])
        .unwrap();
        let kinds: Vec<_> = pollen.readings().iter().map(|r| r.kind()).collect();
        assert_eq!(kinds, [PollenType::Grass, PollenType::Birch]);
        assert_eq!(pollen.readings()[0].grains(), 85.0);
    }

    #[test]
    fn nothing_when_no_type_has_a_value() {
        assert_eq!(Pollen::new(&[(PollenType::Alder, None)]), None);
    }
}
