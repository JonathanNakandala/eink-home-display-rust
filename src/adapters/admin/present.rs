//! What `displayctl` says: the admin API's answers as sentences and a table, for a person at a terminal.
//!
//! Kept apart from the program so what it says can be tested. Times are shown in the zone the caller passes (the
//! local one, for the command), and ages are relative to `now`, so the same answer reads sensibly later.

use chrono::{DateTime, Duration, Utc};

use super::api::{
    CredentialEntry, CredentialList, DisplayEntry, DisplayList, DisplayState, KeySource,
    LayoutList, ViewerEntry, ViewerIssued, ViewerList, ViewerStatus, WindowState,
};

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
            "asked {} ago{}. Check the code on its own panel, then approve it",
            entry
                .waiting_since
                .map_or_else(|| "a while".to_owned(), ago),
            entry
                .profile
                .as_ref()
                .map_or_else(String::new, |profile| format!(" ({profile})"))
        ),
        DisplayState::Approved => {
            "approved. It becomes a member the next time it asks, within a few minutes".to_owned()
        }
        DisplayState::Member => match entry.certificate_not_after {
            Some(end) if entry.certificate_expired => format!(
                "certificate ran out {} ago. It gets a new one by itself when it is next switched on",
                age(now - end)
            ),
            Some(end) if entry.renewal_overdue => format!(
                "certificate until {} ({} left), and it should have been renewed by now. Check that the display is on and can reach the server",
                end.with_timezone(zone).format("%Y-%m-%d"),
                age(end - now)
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
    if entry.state == DisplayState::Member
        && let Some(profile) = &entry.profile
    {
        text.push_str(&format!(" ({profile})"));
    }
    if let Some(layout) = &entry.layout {
        text.push_str(&format!("; layout {layout}"));
    }
    if entry.replacement_waiting && entry.state == DisplayState::Member {
        text.push_str(&format!(
            "; another key asked {} ago to take its name{}. Approve it with the code on the display's own panel, or reject it",
            entry.waiting_since.map_or_else(|| "a while".to_owned(), ago),
            entry
                .replacement_profile
                .as_ref()
                .map_or_else(String::new, |profile| format!(" ({profile})"))
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

/// The layouts there are, as a table, with the default marked.
pub fn describe_layouts(list: &LayoutList) -> String {
    let width = list
        .layouts
        .iter()
        .map(|layout| layout.name.len())
        .max()
        .unwrap_or(0)
        .max("NAME".len());
    let mut lines = vec![format!("{:width$}  DESCRIPTION", "NAME")];
    for layout in &list.layouts {
        lines.push(format!(
            "{:width$}  {}{}",
            layout.name,
            layout.description,
            if layout.default { " (default)" } else { "" }
        ));
    }
    lines.join("\n")
}

/// What was done, in a sentence, once a display's layout was chosen or cleared.
pub fn layout_changed(entry: &DisplayEntry) -> String {
    match &entry.layout {
        Some(layout) => format!(
            "{} will be sent the {layout} layout from its next check-in. A layout it asks for itself (`?layout=`) still wins.",
            entry.name
        ),
        None => format!(
            "{} has no layout of its own, so it is sent the default.",
            entry.name
        ),
    }
}

/// The viewers, as a table.
pub fn describe_viewers<Z: chrono::TimeZone>(
    list: &ViewerList,
    now: DateTime<Utc>,
    zone: &Z,
) -> String
where
    Z::Offset: std::fmt::Display,
{
    if list.viewers.is_empty() {
        return "There are no viewers. Make one with `displayctl viewer create --label NAME`."
            .to_owned();
    }
    let width = list
        .viewers
        .iter()
        .map(|viewer| viewer.label.chars().count())
        .max()
        .unwrap_or(0)
        .max("LABEL".len());
    let mut lines = vec![format!(
        "{:8}  {:width$}  {:12}  DETAIL",
        "ID", "LABEL", "STATE"
    )];
    for viewer in &list.viewers {
        let detail = match viewer.status {
            ViewerStatus::Waiting => format!(
                "link works for {} more; {}",
                viewer
                    .link_expires_at
                    .map_or_else(|| "a while".to_owned(), |at| age(at - now)),
                viewer.scope
            ),
            ViewerStatus::LinkExpired => {
                "its link was not used in time; revoke it and make another".to_owned()
            }
            ViewerStatus::Active => format!(
                "{} {}; {}{}",
                if viewer.refreshing {
                    "session until"
                } else {
                    "until"
                },
                viewer.expires_at.map_or_else(String::new, |at| at
                    .with_timezone(zone)
                    .format("%Y-%m-%d")
                    .to_string()),
                viewer.scope,
                viewer
                    .last_used_at
                    .map_or_else(String::new, |at| format!("; used {} ago", age(now - at)))
            ),
            ViewerStatus::Expired => "its token has run out; make another viewer".to_owned(),
            ViewerStatus::Revoked => "revoked; forget it to free its place".to_owned(),
        };
        lines.push(format!(
            "{:8}  {:width$}  {:12}  {detail}",
            viewer.id,
            viewer.label,
            viewer_label(viewer.status)
        ));
    }
    lines.join("\n")
}

fn viewer_label(status: ViewerStatus) -> &'static str {
    match status {
        ViewerStatus::Waiting => "waiting",
        ViewerStatus::LinkExpired => "link expired",
        ViewerStatus::Active => "active",
        ViewerStatus::Expired => "expired",
        ViewerStatus::Revoked => "revoked",
    }
}

/// What to say when a viewer was made: the link, once, and what it will and won't do.
pub fn viewer_issued<Z: chrono::TimeZone>(
    issued: &ViewerIssued,
    now: DateTime<Utc>,
    zone: &Z,
) -> String
where
    Z::Offset: std::fmt::Display,
{
    let notes: String = issued
        .warnings
        .iter()
        .map(|warning| format!("Note: {warning}.\n"))
        .collect();
    format!(
        "{notes}{}Made the viewer {} ({}), who may look at: {}.\n\n\
         Open this in the browser to sign it in. It works once, and for {} more (until {}), and is not shown again:\n\n  {}\n\n\
         The part after # never leaves the browser. If it is lost or not used in time, revoke the viewer and make another.",
        if notes.is_empty() { "" } else { "\n" },
        issued.viewer.id,
        issued.viewer.label,
        issued.viewer.scope,
        age(issued.link_expires_at - now),
        issued.link_expires_at.with_timezone(zone).format("%H:%M"),
        issued.link
    )
}

/// The quiet zone round a QR code, in modules: the standard asks for four, which is what a camera needs to find the edge.
pub(super) const QUIET_ZONE: usize = 4;

/// The modules of a QR code of `text`, dark ones true, as a square with the quiet zone round it.
pub(super) fn qr_modules(text: &str) -> Result<Vec<Vec<bool>>, qrcode::types::QrError> {
    use qrcode::{Color, EcLevel, QrCode};

    let code = QrCode::with_error_correction_level(text, EcLevel::M)?;
    let width = code.width();
    let colors = code.to_colors();
    let side = width + 2 * QUIET_ZONE;
    Ok((0..side)
        .map(|y| {
            (0..side)
                .map(|x| {
                    x >= QUIET_ZONE
                        && y >= QUIET_ZONE
                        && x - QUIET_ZONE < width
                        && y - QUIET_ZONE < width
                        && colors[(y - QUIET_ZONE) * width + (x - QUIET_ZONE)] == Color::Dark
                })
                .collect()
        })
        .collect())
}

/// A QR code of `text` to print in a terminal: black modules on white, in explicit colours so that it scans whatever the
/// terminal's own theme is (a light code on a dark theme is what a camera often can't read), two rows of modules to a line
/// by way of half blocks. It is for a person to point a camera at, so it holds whatever `text` holds: a one-time code is
/// as secret as it was.
pub fn qr_for_terminal(text: &str) -> Result<String, qrcode::types::QrError> {
    let modules = qr_modules(text)?;
    let side = modules.len();
    let dark = |x: usize, y: usize| modules.get(y).is_some_and(|row| row[x]);
    let mut out = String::new();
    for y in (0..side).step_by(2) {
        let mut last = None;
        for x in 0..side {
            // The upper half of the block is the foreground, the lower half the background.
            let cell = (
                if dark(x, y) { 30 } else { 37 },
                if dark(x, y + 1) { 40 } else { 47 },
            );
            if last != Some(cell) {
                out.push_str(&format!("\x1b[{};{}m", cell.0, cell.1));
                last = Some(cell);
            }
            out.push('\u{2580}');
        }
        out.push_str("\x1b[0m\n");
    }
    Ok(out)
}

/// What was done, in a sentence, once a viewer was revoked or forgotten.
pub fn viewer_changed(entry: &ViewerEntry, forgotten: bool) -> String {
    if forgotten {
        format!(
            "Forgot the viewer {} ({}). Its place is free, and its token, if it had one, no longer works.",
            entry.id, entry.label
        )
    } else {
        format!(
            "Revoked the viewer {} ({}). Its link or token stops working now.",
            entry.id, entry.label
        )
    }
}

/// The keys, as a table: whether each is set and where it came from. Never a key.
pub fn describe_credentials(list: &CredentialList, now: DateTime<Utc>) -> String {
    let width = list
        .credentials
        .iter()
        .map(|entry| entry.name.chars().count())
        .max()
        .unwrap_or(0)
        .max("NAME".len());
    let mut lines = vec![format!("{:width$}  {:7}  DETAIL", "NAME", "STATE")];
    for entry in &list.credentials {
        let detail = match (entry.set, entry.source) {
            (true, Some(KeySource::Config)) => "from the configuration file".to_owned(),
            (true, Some(KeySource::Remembered)) => "set earlier and kept for a restart".to_owned(),
            (true, _) => format!(
                "set{}; {}",
                entry
                    .since
                    .map_or_else(String::new, |at| format!(" {} ago", age(now - at))),
                if entry.remembered {
                    "kept for a restart"
                } else {
                    "in memory only, gone when the server stops"
                }
            ),
            (false, _) if entry.optional => {
                format!("{}; works without one, but slower", entry.description)
            }
            (false, _) => format!("{}; unavailable until it is set", entry.description),
        };
        lines.push(format!(
            "{:width$}  {:7}  {detail}",
            entry.name,
            if entry.set { "set" } else { "not set" },
        ));
    }
    lines.join(
        "
",
    )
}

/// What was done, in a sentence, once a key was set or taken away. Never the key.
pub fn credential_changed(entry: &CredentialEntry, cleared: bool) -> String {
    if cleared {
        return match entry.source {
            Some(KeySource::Config) => format!(
                "Took away the {} key that was set. The one in the configuration file is in use again.",
                entry.name
            ),
            _ if entry.optional => format!(
                "Took away the {} key that was set. There is none now; requests go without one.",
                entry.name
            ),
            _ => format!(
                "Took away the {} key that was set. There is none now, so that source is unavailable until one is set.",
                entry.name
            ),
        };
    }
    format!(
        "Set the {} key. It is used from the next request to the service{}.",
        entry.name,
        if entry.remembered {
            ", and kept in a file only the server's user can read, so it is there after a restart"
        } else {
            ". It is in memory only: it is gone when the server stops"
        }
    )
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
