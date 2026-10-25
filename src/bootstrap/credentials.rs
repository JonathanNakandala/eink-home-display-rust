//! The keys the server uses for other services: where they start, and what is said when one is missing.

use std::sync::Arc;

use crate::application::credentials::Credentials;
use crate::config::application::ApplicationConfig;
use crate::domain::services::clock::Clock;

/// The keys, starting as the configuration file has them (possibly none). The owner can set more while the server runs;
/// see `Credentials`.
pub fn setup_credentials(config: &ApplicationConfig, clock: Arc<dyn Clock>) -> Arc<Credentials> {
    Credentials::new(clock, config.credential_keys())
}

/// Says, for each key that something in use needs and that there is none for, how to set it. Not an error: the server
/// runs, and shows that source as unavailable, until a key is set.
pub fn report_missing_keys(config: &ApplicationConfig, credentials: &Credentials) {
    for name in config.credentials_needed() {
        if !credentials.status(name).is_set() {
            tracing::warn!(
                "There is no key for {name} ({}). Set one while the server runs with `displayctl credential set {name}`, \
                 or put it in the configuration file; until then that source is shown as unavailable",
                name.describe()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono_tz::Tz;

    use super::*;
    use crate::adapters::clock::SystemClock;
    use crate::domain::models::credential::CredentialName;

    fn clock() -> Arc<dyn Clock> {
        Arc::new(SystemClock::new(Tz::UTC))
    }

    #[test]
    fn the_server_starts_with_the_keys_the_file_has() {
        let config = ApplicationConfig::example();

        let credentials = setup_credentials(&config, clock());

        assert!(credentials.list().iter().all(|status| status.is_set()));
    }

    #[test]
    fn a_missing_key_is_said_with_how_to_set_it_and_a_key_that_is_there_is_not_mentioned() {
        crate::captured_log::install();
        let mut config = ApplicationConfig::example();
        // Unique to this test, so that other tests running beside it can't be mistaken for it.
        config.departures[0].name = "NOTICE-TEST".to_owned();
        config.weather.open_weather.as_mut().unwrap().api_key = None;
        let credentials = setup_credentials(&config, clock());
        assert!(!credentials.status(CredentialName::OpenWeather).is_set());
        assert!(credentials.status(CredentialName::OpenLdbws).is_set());

        report_missing_keys(&config, &credentials);

        let said = crate::captured_log::lines_containing(
            "There is no key for open-weather (OpenWeather API key (weather)). Set one",
        );
        assert!(!said.is_empty());
        assert!(
            said[0].contains("displayctl credential set open-weather"),
            "{said:?}"
        );
        assert!(crate::captured_log::lines_containing("There is no key for open-ldbws").is_empty());
    }
}
