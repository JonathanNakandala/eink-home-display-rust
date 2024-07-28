pub struct WeatherInformation {
    temperature: i8
}

impl WeatherInformation {
    pub fn new(temperature: i8) -> Self {
        WeatherInformation {
            temperature
        }
    }

    pub fn temperature(&self) -> i8 {
        self.temperature
    }
}