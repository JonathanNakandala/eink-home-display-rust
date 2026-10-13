//! The ways the same data can be laid out on a display. Each is rendered on every run, so a display can
//! be pointed at any of them.

use std::fmt;

use chrono::{DateTime, Datelike, NaiveTime, Weekday};
use chrono_tz::Tz;
use serde::Serialize;

/// What a layout is called: lower-case letters, digits and hyphens, starting with a letter, so it is safe in a
/// file name and a query string as it stands.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayoutName(String);

const LONGEST: usize = 32;

impl LayoutName {
    /// The name, or why it isn't one.
    pub fn parse(name: &str) -> Result<Self, String> {
        let mut chars = name.chars();
        let starts_well = chars.next().is_some_and(|c| c.is_ascii_lowercase());
        let rest_well = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !starts_well || !rest_well || name.len() > LONGEST {
            return Err(format!(
                "A layout name is up to {LONGEST} lower-case letters, digits and hyphens, starting with a letter"
            ));
        }
        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LayoutName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub name: LayoutName,
    /// A line saying what it shows, for whoever is choosing.
    pub description: String,
}

impl Layout {
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            name: LayoutName::parse(name).expect("a layout built in has a valid name"),
            description: description.to_owned(),
        }
    }
}

/// When a layout is wanted: on these days, from one time of day to another, in the server's own zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutRule {
    pub layout: LayoutName,
    /// The days it applies on; all of them if empty. For a rule that runs past midnight, the day it starts on.
    pub days: Vec<Weekday>,
    /// From this time (inclusive)...
    pub from: NaiveTime,
    /// ...until this one (exclusive). Earlier than `from` means the rule runs overnight, into the next day.
    pub until: NaiveTime,
}

impl LayoutRule {
    /// The rule, or why it can't be one: `from` and `until` are the same moment, which is a whole day or none.
    pub fn new(
        layout: LayoutName,
        days: Vec<Weekday>,
        from: NaiveTime,
        until: NaiveTime,
    ) -> Result<Self, String> {
        if from == until {
            return Err(format!(
                "The rule for {layout} starts and ends at {}: give it a start before its end, or an end before its start to run overnight",
                from.format("%H:%M")
            ));
        }
        Ok(Self {
            layout,
            days,
            from,
            until,
        })
    }

    fn on(&self, day: Weekday) -> bool {
        self.days.is_empty() || self.days.contains(&day)
    }

    /// Whether the rule is in force at `now`.
    pub fn applies(&self, now: DateTime<Tz>) -> bool {
        let (time, today) = (now.time(), now.weekday());
        if self.from < self.until {
            self.on(today) && time >= self.from && time < self.until
        } else {
            // Overnight: the evening of a listed day, or the small hours after one.
            (self.on(today) && time >= self.from) || (self.on(today.pred()) && time < self.until)
        }
    }
}

/// Which layout a display with none chosen gets, by the time: the first rule in force wins, and with none in force it
/// is the default.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LayoutSchedule(Vec<LayoutRule>);

impl LayoutSchedule {
    pub fn new(rules: Vec<LayoutRule>) -> Self {
        Self(rules)
    }

    pub fn rules(&self) -> &[LayoutRule] {
        &self.0
    }

    /// The layout wanted at `now`, if a rule says.
    pub fn at(&self, now: DateTime<Tz>) -> Option<&LayoutName> {
        self.0
            .iter()
            .find(|rule| rule.applies(now))
            .map(|rule| &rule.layout)
    }
}

/// The layouts there are. The first is the default, and with a schedule (`with_schedule`) the default a display with
/// none chosen is sent can change by the time of day; what is shown on a panel that is driven directly follows that too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layouts {
    layouts: Vec<Layout>,
    schedule: LayoutSchedule,
}

impl Layouts {
    /// `layouts` in order, the first the default, or why they can't be: none, or two with a name.
    pub fn new(layouts: Vec<Layout>) -> Result<Self, String> {
        if layouts.is_empty() {
            return Err("There must be at least one layout".to_owned());
        }
        for (i, layout) in layouts.iter().enumerate() {
            if layouts[..i].iter().any(|other| other.name == layout.name) {
                return Err(format!("There are two layouts called {}", layout.name));
            }
        }
        Ok(Self {
            layouts,
            schedule: LayoutSchedule::default(),
        })
    }

    /// These layouts, with `schedule` deciding the default by the time of day. A rule for a layout there isn't is an
    /// error, since it could never be shown.
    pub fn with_schedule(mut self, schedule: LayoutSchedule) -> Result<Self, String> {
        for rule in schedule.rules() {
            if self.find(rule.layout.as_str()).is_none() {
                return Err(format!(
                    "The schedule names the layout {}, which there is not (there are: {})",
                    rule.layout,
                    self.layouts
                        .iter()
                        .map(|layout| layout.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        self.schedule = schedule;
        Ok(self)
    }

    /// The first layout: the one with no schedule and no choice. A schedule can put another in its place for a while
    /// (see `current`).
    pub fn default_layout(&self) -> &Layout {
        &self.layouts[0]
    }

    /// The layout a display with none chosen is sent at `now`: the schedule's, or else the default.
    pub fn current(&self, now: DateTime<Tz>) -> &Layout {
        self.schedule
            .at(now)
            .and_then(|name| self.find(name.as_str()))
            .unwrap_or_else(|| self.default_layout())
    }

    pub fn schedule(&self) -> &LayoutSchedule {
        &self.schedule
    }

    pub fn all(&self) -> &[Layout] {
        &self.layouts
    }

    /// The layout called `name`, if there is one.
    pub fn find(&self, name: &str) -> Option<&Layout> {
        self.layouts
            .iter()
            .find(|layout| layout.name.as_str() == name)
    }
}

/// The name the one layout of a program with no others goes by.
pub const DEFAULT_LAYOUT: &str = "dashboard";

impl Default for Layouts {
    /// Just the dashboard.
    fn default() -> Self {
        Self {
            layouts: vec![Layout::new(DEFAULT_LAYOUT, "Weather and departures")],
            schedule: LayoutSchedule::default(),
        }
    }
}

/// A layout as the API lists it.
#[derive(Debug, Clone, Serialize)]
pub struct LayoutSummary {
    pub name: String,
    pub description: String,
    pub default: bool,
    /// What a display with none chosen is sent now: the default, unless the schedule says another.
    pub current: bool,
}

impl Layouts {
    /// The layouts as the API lists them, with the one a display with none chosen is sent at `now` marked.
    pub fn summaries(&self, now: DateTime<Tz>) -> Vec<LayoutSummary> {
        let current = self.current(now).name.clone();
        self.layouts
            .iter()
            .enumerate()
            .map(|(i, layout)| LayoutSummary {
                name: layout.name.to_string(),
                description: layout.description.clone(),
                default: i == 0,
                current: layout.name == current,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_in_a_path_and_a_query() {
        for good in ["dashboard", "departures", "a", "big-clock-2"] {
            assert!(LayoutName::parse(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "Dashboard",
            "2fast",
            "-x",
            "a b",
            "a/b",
            "a.b",
            "..",
            "é",
            &"a".repeat(33),
        ] {
            assert!(LayoutName::parse(bad).is_err(), "{bad:?}");
        }
        assert!(LayoutName::parse(&"a".repeat(32)).is_ok());
    }

    #[test]
    fn the_first_layout_is_the_default_and_names_are_unique() {
        let layouts = Layouts::new(vec![
            Layout::new("one", "First"),
            Layout::new("two", "Second"),
        ])
        .unwrap();
        assert_eq!(layouts.default_layout().name.as_str(), "one");
        assert_eq!(layouts.find("two").unwrap().description, "Second");
        assert!(layouts.find("three").is_none());
        let summaries = layouts.summaries(at(2026, 10, 12, 12, 0));
        assert_eq!(
            summaries.iter().map(|s| s.default).collect::<Vec<_>>(),
            [true, false]
        );
        assert_eq!(
            summaries.iter().map(|s| s.current).collect::<Vec<_>>(),
            [true, false]
        );

        assert!(Layouts::new(vec![]).is_err());
        assert!(Layouts::new(vec![Layout::new("one", "a"), Layout::new("one", "b")]).is_err());
    }

    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    /// 2026-10-12 is a Monday.
    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Tz> {
        London
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
    }

    fn time(hour: u32, minute: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
    }

    fn rule(layout: &str, days: &[Weekday], from: NaiveTime, until: NaiveTime) -> LayoutRule {
        LayoutRule::new(
            LayoutName::parse(layout).unwrap(),
            days.to_vec(),
            from,
            until,
        )
        .unwrap()
    }

    fn two() -> Layouts {
        Layouts::new(vec![
            Layout::new("dashboard", "Everything"),
            Layout::new("departures", "Trains"),
        ])
        .unwrap()
    }

    const WEEKDAYS: [Weekday; 5] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
    ];

    #[test]
    fn a_rule_applies_from_its_start_up_to_but_not_including_its_end_on_its_days() {
        let commute = rule("departures", &WEEKDAYS, time(6, 30), time(9, 30));

        assert!(!commute.applies(at(2026, 10, 12, 6, 29)));
        assert!(commute.applies(at(2026, 10, 12, 6, 30)));
        assert!(commute.applies(at(2026, 10, 12, 9, 29)));
        assert!(!commute.applies(at(2026, 10, 12, 9, 30)));
        // The weekend (the 17th is a Saturday) isn't one of its days.
        assert!(!commute.applies(at(2026, 10, 17, 8, 0)));
        // With no days listed, every day is.
        let always = rule("departures", &[], time(6, 30), time(9, 30));
        assert!(always.applies(at(2026, 10, 17, 8, 0)));
    }

    #[test]
    fn a_rule_that_ends_before_it_starts_runs_overnight_from_the_day_it_starts_on() {
        // Friday evening to Saturday morning, and not Saturday evening.
        let late = rule("departures", &[Weekday::Fri], time(22, 0), time(6, 0));

        assert!(late.applies(at(2026, 10, 16, 22, 0)), "Friday 22:00");
        assert!(late.applies(at(2026, 10, 16, 23, 59)));
        assert!(late.applies(at(2026, 10, 17, 0, 0)), "Saturday midnight");
        assert!(late.applies(at(2026, 10, 17, 5, 59)));
        assert!(!late.applies(at(2026, 10, 17, 6, 0)));
        assert!(!late.applies(at(2026, 10, 17, 22, 0)), "Saturday evening");
        assert!(!late.applies(at(2026, 10, 16, 5, 0)), "Friday small hours");
    }

    #[test]
    fn a_rule_that_starts_and_ends_together_is_refused() {
        let error = LayoutRule::new(
            LayoutName::parse("departures").unwrap(),
            vec![],
            time(8, 0),
            time(8, 0),
        )
        .unwrap_err();
        assert!(
            error.contains("departures") && error.contains("08:00"),
            "{error}"
        );
    }

    #[test]
    fn the_first_rule_in_force_wins_and_with_none_the_default_is_current() {
        let layouts = two()
            .with_schedule(LayoutSchedule::new(vec![
                rule("departures", &WEEKDAYS, time(6, 30), time(9, 30)),
                rule("dashboard", &[], time(8, 0), time(8, 30)),
            ]))
            .unwrap();

        // Both apply at 08:15 on a weekday: the first listed.
        assert_eq!(
            layouts.current(at(2026, 10, 12, 8, 15)).name.as_str(),
            "departures"
        );
        // Only the second at the weekend.
        assert_eq!(
            layouts.current(at(2026, 10, 17, 8, 15)).name.as_str(),
            "dashboard"
        );
        // Neither.
        assert_eq!(
            layouts.current(at(2026, 10, 12, 12, 0)).name.as_str(),
            "dashboard"
        );
        // The default is the default whatever the time.
        assert_eq!(layouts.default_layout().name.as_str(), "dashboard");
        let at_eight = layouts.summaries(at(2026, 10, 12, 8, 15));
        assert_eq!(
            at_eight
                .iter()
                .map(|s| (s.default, s.current))
                .collect::<Vec<_>>(),
            [(true, false), (false, true)]
        );
    }

    #[test]
    fn a_rule_for_a_layout_that_does_not_exist_is_refused_naming_what_does() {
        let error = two()
            .with_schedule(LayoutSchedule::new(vec![rule(
                "nonesuch",
                &[],
                time(6, 0),
                time(7, 0),
            )]))
            .unwrap_err();
        assert!(
            error.contains("nonesuch") && error.contains("dashboard, departures"),
            "{error}"
        );
    }

    #[test]
    fn the_time_is_the_wall_clock_in_the_servers_zone_across_a_clock_change() {
        // Clocks went back on Sunday 2026-10-25: 06:30 is 06:30 on both sides of it.
        let commute = rule("departures", &[], time(6, 30), time(9, 30));
        assert!(commute.applies(at(2026, 10, 24, 7, 0)));
        assert!(commute.applies(at(2026, 10, 26, 7, 0)));
    }
}
