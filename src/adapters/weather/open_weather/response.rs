use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[allow(unused)]
pub struct OpenWeatherResponse {
    pub main: OpenWeatherMainResponse,
    pub name: String,
    pub visibility: i16,
    pub wind: OpenWeatherWindResponse,
}

#[derive(Deserialize, Debug)]
#[allow(unused)]
pub struct OpenWeatherWindResponse {
    pub speed: f32,
    pub deg: i16,
}

#[derive(Deserialize, Debug)]
#[allow(unused)]
pub struct OpenWeatherMainResponse {
    pub temp: f64,
    pub feels_like: f32,
    pub temp_min: f32,
    pub temp_max: f32,
    pub pressure: i32,
    pub humidity: i32,
    pub sea_level: i32,
    pub grnd_level: i32,
}