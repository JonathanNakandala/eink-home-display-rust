use eink_home_display_rust::adapters::weather::open_weather::open_weather_weather_service::OpenWeatherWeatherServiceAdapter;
use eink_home_display_rust::application::Application;

fn main() -> anyhow::Result<()> {
    let application = Application::new(OpenWeatherWeatherServiceAdapter::new());

    let result = application.run()?;
    
    println!("Yay!! you're temperature is {}degC", result.weather_information().temperature());
    
    Ok(())
}
