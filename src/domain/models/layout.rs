//! The ways the same data can be laid out on a display. Each is rendered on every run, so a display can
//! be pointed at any of them.

use std::fmt;

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

/// The layouts there are. The first is the default: what a display is sent when it doesn't ask for one, and the
/// only one a panel that is driven directly shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layouts(Vec<Layout>);

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
        Ok(Self(layouts))
    }

    pub fn default_layout(&self) -> &Layout {
        &self.0[0]
    }

    pub fn all(&self) -> &[Layout] {
        &self.0
    }

    /// The layout called `name`, if there is one.
    pub fn find(&self, name: &str) -> Option<&Layout> {
        self.0.iter().find(|layout| layout.name.as_str() == name)
    }
}

/// The name the one layout of a program with no others goes by.
pub const DEFAULT_LAYOUT: &str = "dashboard";

impl Default for Layouts {
    /// Just the dashboard.
    fn default() -> Self {
        Self(vec![Layout::new(DEFAULT_LAYOUT, "Weather and departures")])
    }
}

/// A layout as the API lists it.
#[derive(Debug, Clone, Serialize)]
pub struct LayoutSummary {
    pub name: String,
    pub description: String,
    pub default: bool,
}

impl Layouts {
    pub fn summaries(&self) -> Vec<LayoutSummary> {
        self.0
            .iter()
            .enumerate()
            .map(|(i, layout)| LayoutSummary {
                name: layout.name.to_string(),
                description: layout.description.clone(),
                default: i == 0,
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
        let summaries = layouts.summaries();
        assert_eq!(
            summaries.iter().map(|s| s.default).collect::<Vec<_>>(),
            [true, false]
        );

        assert!(Layouts::new(vec![]).is_err());
        assert!(Layouts::new(vec![Layout::new("one", "a"), Layout::new("one", "b")]).is_err());
    }
}
