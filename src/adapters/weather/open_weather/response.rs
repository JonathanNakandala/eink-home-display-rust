use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[allow(unused)]
pub struct OpenWeatherResponse {
    pub main: OpenWeatherMainResponse,
}

#[derive(Deserialize, Debug)]
#[allow(unused)]
pub struct OpenWeatherMainResponse {
    pub temp: f64,
}
