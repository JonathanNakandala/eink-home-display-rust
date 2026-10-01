#[derive(Debug, Clone, PartialEq, Eq, derive_new::new)]
pub struct Arrival {
    pub line: String,
    pub destination: String,
    pub towards: String,
    /// Platform for stations; stop letter for bus stops.
    pub platform: String,
    pub current_location: String,
    pub seconds_to_arrival: u32,
    /// The stop or station the prediction is for, e.g. "Turnpike Lane Underground Station".
    pub station_name: String,
}
