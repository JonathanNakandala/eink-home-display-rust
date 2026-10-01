#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopKind {
    Bus,
    Underground,
}

#[derive(Debug, Clone, PartialEq, derive_new::new)]
pub struct StopPoint {
    pub id: String,
    pub name: String,
    /// Bus stop letter/indicator (e.g. "Stop B"); absent for stations.
    pub indicator: Option<String>,
    /// Metres from the search point; absent for name searches.
    pub distance: Option<f64>,
    pub lines: Vec<String>,
    pub latitude: f64,
    pub longitude: f64,
}
