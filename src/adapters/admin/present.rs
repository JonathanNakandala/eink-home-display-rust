//! What `displayctl` says: the admin API's answers as sentences and a table, for a person at a terminal.
//!
//! Kept apart from the program so what it says can be tested. Times are shown in the zone the caller passes (the
//! local one, for the command), and ages are relative to `now`, so the same answer reads sensibly later.

use chrono::{DateTime, Duration, Utc};

use super::api::{DisplayEntry, DisplayList, DisplayState, WindowState};

/// How long ago, or how long to go, in the largest units that matter: `just now`, `3 min`, `2 h 5 min`, `3 days`.
pub fn age(span: Duration) -> String {
    let seconds = span.num_seconds().max(0);
    let (days, hours, minutes) = (
        seconds / 86_400,
        seconds % 86_400 / 3_600,
        seconds % 3_600 / 60,
    );
    match (days, hours, minutes) {
        (0, 0, 0) => "less than a minute".to_owned(),
        (0, 0, m) => format!("{m} min"),
        (0, h, 0) => format!("{h} h"),
        (0, h, m) => format!("{h} h {m} min"),
        (1, 0, _) => "1 day".to_owned(),
        (d, 0, _) => format!("{d} days"),
        (1, h, _) => format!("1 day {h} h"),
        (d, h, _) => format!("{d} days {h} h"),
    }
}

/// The state of the window in a sentence for a person, with times in `zone`.
pub fn describe_window<Z: chrono::TimeZone>(
    state: &WindowState,
    now: chrono::DateTime<chrono::Utc>,
    zone: &Z,
) -> String
where
    Z::Offset: std::fmt::Display,
{
    match state.closes_at {
        Some(closes) if state.open => {
            let left = (closes - now).num_seconds().max(0);
            let (minutes, seconds) = (left / 60, left % 60);
            format!(
                "The pairing window is open until {} ({minutes} min {seconds:02} s from now). A display that has not joined can ask now.",
                closes.with_timezone(zone).format("%H:%M:%S")
            )
        }
        _ => "The pairing window is closed. Open it with `displayctl window open` to let a display ask to join."
            .to_owned(),
    }
}

/// Where a display stands, in a phrase that says what to do about it, if anything.
pub fn describe_entry<Z: chrono::TimeZone>(
    entry: &DisplayEntry,
    now: DateTime<Utc>,
    zone: &Z,
) -> String
where
    Z::Offset: std::fmt::Display,
{
    let ago = |at: DateTime<Utc>| age(now - at);
    let mut text = match entry.state {
        DisplayState::Waiting => format!(
            "asked {} ago. Check the code on its own panel, then approve it",
            entry
                .waiting_since
                .map_or_else(|| "a while".to_owned(), ago)
        ),
        DisplayState::Approved => {
            "approved. It becomes a member the next time it asks, within a few minutes".to_owned()
        }
        DisplayState::Member => match entry.certificate_not_after {
            Some(end) if entry.certificate_expired => format!(
                "certificate ran out {} ago. It gets a new one by itself when it is next switched on",
                age(now - end)
            ),
            Some(end) => format!(
                "certificate until {} ({} left)",
                end.with_timezone(zone).format("%Y-%m-%d"),
                age(end - now)
            ),
            None => "member".to_owned(),
        },
        DisplayState::Rejected => "turned down. Forget it to let it ask again".to_owned(),
        DisplayState::Revoked => "revoked. Forget it to let it ask again".to_owned(),
    };
    if entry.changing_keys {
        text.push_str("; changing to a new key");
    }
    if entry.replacement_waiting && entry.state == DisplayState::Member {
        text.push_str(&format!(
            "; another key asked {} ago to take its name. Approve it with the code on the display's own panel, or reject it",
            entry.waiting_since.map_or_else(|| "a while".to_owned(), ago)
        ));
    }
    text
}

/// The label for a state, as the owner reads it.
pub fn label(state: DisplayState) -> &'static str {
    match state {
        DisplayState::Waiting => "waiting",
        DisplayState::Approved => "approved",
        DisplayState::Member => "member",
        DisplayState::Rejected => "rejected",
        DisplayState::Revoked => "revoked",
    }
}

/// Every display as a table: name, state, and what to do about it.
pub fn describe_list<Z: chrono::TimeZone>(
    list: &DisplayList,
    now: DateTime<Utc>,
    zone: &Z,
) -> String
where
    Z::Offset: std::fmt::Display,
{
    if list.displays.is_empty() {
        return "No display has asked to join yet. Open the pairing window with `displayctl window open`, then \
                start the display."
            .to_owned();
    }
    let width = list
        .displays
        .iter()
        .map(|entry| entry.name.len())
        .max()
        .unwrap_or(0)
        .max("NAME".len());
    let mut lines = vec![format!("{:width$}  {:8}  DETAIL", "NAME", "STATE")];
    for entry in &list.displays {
        lines.push(format!(
            "{:width$}  {:8}  {}",
            entry.name,
            label(entry.state),
            describe_entry(entry, now, zone)
        ));
    }
    lines.join("\n")
}

/// What was done, in a sentence, once approving worked.
pub fn approved(entry: &DisplayEntry) -> String {
    if entry.state == DisplayState::Member {
        format!(
            "Approved the new key for {}. It takes the name over the next time it asks, within a few minutes, and the \
             old key stops being accepted then.",
            entry.name
        )
    } else {
        format!(
            "Approved {}. It becomes a member the next time it asks, within a few minutes.",
            entry.name
        )
    }
}

/// What was done, once turning a display down worked.
pub fn rejected(entry: &DisplayEntry) -> String {
    if entry.state == DisplayState::Member {
        format!(
            "Turned down the other key's request for {}'s name. {} carries on as it was.",
            entry.name, entry.name
        )
    } else {
        format!(
            "Turned down {}. It stays turned down until it is forgotten.",
            entry.name
        )
    }
}

pub fn revoked(entry: &DisplayEntry) -> String {
    format!(
        "Revoked {}. It is turned away from its very next request.",
        entry.name
    )
}

pub fn forgotten(entry: &DisplayEntry) -> String {
    format!(
        "Forgot {}. It is turned away, and can ask to join again while the pairing window is open.",
        entry.name
    )
}
