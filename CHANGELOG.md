# Changelog

## [0.2.0](https://github.com/JonathanNakandala/eink-home-display-rust/compare/eink-home-display-rust-v0.1.0...eink-home-display-rust-v0.2.0) (2026-10-01)


### ⚠ BREAKING CHANGES

* Decouple image rendering from the display with a DisplayProfile ([#19](https://github.com/JonathanNakandala/eink-home-display-rust/issues/19))
* TfL stops/arrivals and per-board departures providers ([#17](https://github.com/JonathanNakandala/eink-home-display-rust/issues/17))

### Features

* Add a render command and rework the dashboard layout ([a7015e2](https://github.com/JonathanNakandala/eink-home-display-rust/commit/a7015e2c3cc74759c6231a2ad704ee1765953739))
* Add Open-Meteo as a weather provider ([2ff77e5](https://github.com/JonathanNakandala/eink-home-display-rust/commit/2ff77e5fcae30f6e2ddf18014f007701cca27204))
* add sunrise and sunset times ([568e2ba](https://github.com/JonathanNakandala/eink-home-display-rust/commit/568e2ba558ede6b9b61a37533ef838cc2bf7c522))
* Add the reTerminal E1003 as a display and render per display type ([451ffb1](https://github.com/JonathanNakandala/eink-home-display-rust/commit/451ffb190ae6832bcc3d282895c584c7432e467f))
* Add travel_minutes to leave off services you can't catch ([d7859f5](https://github.com/JonathanNakandala/eink-home-display-rust/commit/d7859f5cf2b1efae8da42c0bb267ded8767edf9f))
* Complete the Waveshare 7.5" V2 e-ink display adapter ([#18](https://github.com/JonathanNakandala/eink-home-display-rust/issues/18)) ([1a66509](https://github.com/JonathanNakandala/eink-home-display-rust/commit/1a6650908d0337f25037a28aac203e4924f068c9))
* **dashboard:** rain chart and one line temperature ([fca537e](https://github.com/JonathanNakandala/eink-home-display-rust/commit/fca537ecce703b4a45a0056e30fa51643a699d28))
* Decouple image rendering from the display with a DisplayProfile ([#19](https://github.com/JonathanNakandala/eink-home-display-rust/issues/19)) ([fb8f1b8](https://github.com/JonathanNakandala/eink-home-display-rust/commit/fb8f1b86e5158c4564e69d10cb4e1f7f01c8ad97))
* feels like temperature ([1f70991](https://github.com/JonathanNakandala/eink-home-display-rust/commit/1f70991a09821add3b1279d5e084726c14d03fc5))
* Fetch pollen counts from Open-Meteo, off by default ([2228164](https://github.com/JonathanNakandala/eink-home-display-rust/commit/2228164fa955d521754a3ad75f492234dbb86a95))
* Live departures: National Rail REST API for northbound/southbound boards ([#15](https://github.com/JonathanNakandala/eink-home-display-rust/issues/15)) ([8908150](https://github.com/JonathanNakandala/eink-home-display-rust/commit/8908150a29b0a349a45937886016c43ef2457b16))
* Show an upcoming precipitation chart on the weather column ([00ab2f6](https://github.com/JonathanNakandala/eink-home-display-rust/commit/00ab2f6c940ecf5f86bd6455abb8e6e024d04130))
* Show European air quality on the weather column ([241ff9b](https://github.com/JonathanNakandala/eink-home-display-rust/commit/241ff9bad9e047c1af8e2d74138b535e297579ac))
* Show scheduled and expected times, countdowns and station names ([966905d](https://github.com/JonathanNakandala/eink-home-display-rust/commit/966905dc3e1939e3bf8703d6f6865136ffa39281))
* Show the day's peak UV index on the weather column ([fd3aff6](https://github.com/JonathanNakandala/eink-home-display-rust/commit/fd3aff61caf920532becf6b37aa1e50bcf05fa52))
* TfL stops/arrivals and per-board departures providers ([#17](https://github.com/JonathanNakandala/eink-home-display-rust/issues/17)) ([c52be9c](https://github.com/JonathanNakandala/eink-home-display-rust/commit/c52be9c054eee021eb500f994a43c8bdb01e2289))
* Use an installed Chrome and render at the exact panel size ([d8fbfd4](https://github.com/JonathanNakandala/eink-home-display-rust/commit/d8fbfd49664b0ea0202a6188065abad6f1fb0a30))


### Bug Fixes

* Build the page's file:// URL with the url crate ([d22be51](https://github.com/JonathanNakandala/eink-home-display-rust/commit/d22be51270914a7f1c0d8af4c70fb6ecef116490))
