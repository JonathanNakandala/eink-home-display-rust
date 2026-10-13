//! `GET /layouts`, and the `layout` query that `/image`, `/plan` and `/refresh` take: which of the layouts a
//! request is about.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::health::version_of_layout;
use super::{Published, server_error};
use crate::domain::models::device_id::DeviceId;
use crate::domain::models::layout::Layout;

/// Which layout a request is about. Optional: without it, the default.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct Wanted {
    /// The layout to use, one of the names `/layouts` lists. The default layout if left out.
    #[param(value_type = Option<String>, max_length = 32, pattern = "^[a-z][a-z0-9-]*$", example = "departures")]
    layout: Option<String>,
}

impl Wanted {
    /// The layout for a request: the one it asks for, else the one the owner chose for the display asking, else the
    /// default. Or the answer to give when the one asked for does not exist: a name that isn't one is the same as a
    /// name nothing has, not found, with what is.
    pub(super) async fn resolve<'a>(
        &self,
        published: &'a Published,
        caller: Option<&DeviceId>,
    ) -> Result<&'a Layout, (StatusCode, String)> {
        let Some(name) = self.layout.as_deref() else {
            return Ok(chosen_or_default(published, caller).await);
        };
        published.layouts.find(name).ok_or_else(|| {
            let known: Vec<_> = published
                .layouts
                .all()
                .iter()
                .map(|layout| layout.name.as_str())
                .collect();
            // The name is not repeated back: it is whatever the caller sent.
            (
                StatusCode::NOT_FOUND,
                format!("There is no such layout. Layouts: {}", known.join(", ")),
            )
        })
    }
}

/// The layout the owner chose for `caller`, or the default if they chose none, or `caller` is not named, or what they
/// chose has since gone.
async fn chosen_or_default<'a>(published: &'a Published, caller: Option<&DeviceId>) -> &'a Layout {
    let chosen = match (caller, &published.handles.members) {
        (Some(device), Some(members)) => members.layout_of(device).await,
        _ => None,
    };
    match chosen {
        Some(name) => published.layouts.find(name.as_str()).unwrap_or_else(|| {
            log::warn!(
                "The layout chosen for {} is {name}, which there is no longer; sending the default",
                caller.map(|device| device.to_string()).unwrap_or_default()
            );
            published.layouts.default_layout()
        }),
        None => published.layouts.default_layout(),
    }
}

/// A layout, and where its image has got to.
#[derive(Serialize, ToSchema)]
pub(super) struct LayoutEntry {
    /// What to put after `?layout=`.
    #[schema(examples("dashboard"))]
    name: String,
    /// What the layout shows.
    #[schema(examples("Departures with the weather beside them"))]
    description: String,
    /// Whether this is the layout a request without `layout` gets.
    default: bool,
    /// The version `/plan` reports for this layout's image, or absent before it has been rendered.
    #[schema(examples(1791463200))]
    version: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct LayoutList {
    layouts: Vec<LayoutEntry>,
}

/// The layouts there are, the default first.
#[utoipa::path(
    get,
    path = "/layouts",
    tag = "displays",
    responses(
        (
            status = 200,
            description = "The layouts a request can ask for with `layout`.",
            body = LayoutList,
            content_type = "application/json",
            headers(("cache-control" = String, description = "Always `no-store`."))
        ),
        (status = 500, description = "The server could not work its state out.")
    )
)]
pub(super) async fn layouts(State(published): State<Arc<Published>>) -> Response {
    let mut entries = Vec::new();
    for summary in published.layouts.summaries() {
        let version = match published.layouts.find(&summary.name) {
            Some(layout) => match version_of_layout(&published, layout).await {
                Ok(version) => version,
                Err(e) => return server_error("inspect", e),
            },
            None => None,
        };
        entries.push(LayoutEntry {
            name: summary.name,
            description: summary.description,
            default: summary.default,
            version,
        });
    }
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(LayoutList { layouts: entries }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::super::testing::{publish, publish_layout, start, start_with_two_layouts};
    use crate::domain::models::display::ImageFormat;

    async fn body(url: String) -> (u16, String) {
        let response = reqwest::get(url).await.unwrap();
        (response.status().as_u16(), response.text().await.unwrap())
    }

    #[tokio::test]
    async fn the_layouts_are_listed_with_the_default_first_and_where_each_has_got_to() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start_with_two_layouts(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();

        let (status, text) = body(format!("{base}/layouts")).await;

        assert_eq!(status, 200);
        let list: serde_json::Value = serde_json::from_str(&text).unwrap();
        let layouts = list["layouts"].as_array().unwrap();
        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[0]["name"], "dashboard");
        assert_eq!(layouts[0]["default"], true);
        assert!(layouts[0]["version"].as_u64().unwrap() > 0);
        assert_eq!(layouts[1]["name"], "departures");
        assert_eq!(layouts[1]["default"], false);
        // Not rendered yet, so no version.
        assert!(layouts[1]["version"].is_null());
    }

    #[tokio::test]
    async fn the_image_follows_the_layout_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start_with_two_layouts(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"all of it")
            .await
            .unwrap();
        publish_layout(tmp.path(), "departures", ImageFormat::Bmp, b"trains")
            .await
            .unwrap();

        assert_eq!(body(format!("{base}/image")).await.1, "all of it");
        assert_eq!(
            body(format!("{base}/image?layout=dashboard")).await.1,
            "all of it"
        );
        assert_eq!(
            body(format!("{base}/image?layout=departures")).await.1,
            "trains"
        );
    }

    #[tokio::test]
    async fn a_layout_nothing_has_is_not_found_and_the_answer_says_what_there_is() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start_with_two_layouts(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();

        for path in ["image", "plan"] {
            let (status, text) = body(format!("{base}/{path}?layout=nonesuch")).await;
            assert_eq!(status, 404, "{path}");
            assert!(text.contains("dashboard, departures"), "{path}: {text}");
            // What the caller sent is not echoed back.
            assert!(!text.contains("nonesuch"), "{path}: {text}");
        }
        // A name that could never be one is treated the same, not as a different kind of error.
        assert_eq!(body(format!("{base}/image?layout=../etc")).await.0, 404);
    }

    #[tokio::test]
    async fn a_layout_not_yet_rendered_has_no_image_or_plan_though_the_default_does() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start_with_two_layouts(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();

        assert_eq!(body(format!("{base}/plan")).await.0, 200);
        for path in ["image", "plan"] {
            let (status, text) = body(format!("{base}/{path}?layout=departures")).await;
            assert_eq!(status, 404, "{path}");
            assert!(text.contains("No image has been rendered yet"), "{text}");
        }
    }

    #[tokio::test]
    async fn the_plan_is_for_the_layout_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start_with_two_layouts(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        publish_layout(tmp.path(), "departures", ImageFormat::Bmp, b"b")
            .await
            .unwrap();
        let file = std::fs::File::options()
            .write(true)
            .open(tmp.path().join("departures").join("image.bmp"))
            .unwrap();
        file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(1000))
            .unwrap();

        let plan = |query: String| {
            let url = format!("{base}/plan{query}");
            async move { serde_json::from_str::<serde_json::Value>(&body(url).await.1).unwrap() }
        };
        let (default, other) = (
            plan(String::new()).await,
            plan("?layout=departures".into()).await,
        );

        assert!(other["age_seconds"].as_u64().unwrap() >= 1000);
        assert!(default["age_seconds"].as_u64().unwrap() < 100);
        assert_ne!(default["version"], other["version"]);
        // What a display has is judged against the layout it is asking about.
        let have = other["version"].as_u64().unwrap();
        let same = plan(format!("?layout=departures&have={have}")).await;
        assert_eq!(same["changed"], false);
        let elsewhere = plan(format!("?have={have}")).await;
        assert_eq!(elsewhere["changed"], true);
    }

    #[tokio::test]
    async fn with_only_the_default_layout_nothing_changes_for_a_caller_who_never_asks() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();

        assert_eq!(body(format!("{base}/image")).await.1, "a");
        let (_, text) = body(format!("{base}/layouts")).await;
        let list: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(list["layouts"].as_array().unwrap().len(), 1);
    }

    use std::sync::Arc;

    use super::super::testing::start_layouts;
    use crate::adapters::certificate_authority::{self, Create};
    use crate::adapters::clock::SystemClock;
    use crate::adapters::pairing_store::{FilePairingStore, Missing};
    use crate::adapters::published_images::DirectoryImages;
    use crate::application::enrollment::{Enrollment, EnrollmentPolicy};
    use crate::domain::models::device_id::DeviceId;
    use crate::domain::models::layout::{DEFAULT_LAYOUT, Layout, LayoutName, Layouts};
    use crate::domain::models::pairing::{Pairing, PairingState, PublicKey};
    use crate::domain::services::pairing_store::PairingStore;
    use chrono_tz::Europe::London;

    /// A server with two layouts, in which `kitchen` is a member, and its enrollment to choose layouts with.
    async fn with_a_member(directory: &std::path::Path) -> (String, Arc<Enrollment>) {
        let pki = tempfile::tempdir().unwrap();
        let authority =
            Arc::new(certificate_authority::open(pki.path(), Create::IfMissing).unwrap());
        let store = Arc::new(
            FilePairingStore::open(pki.path(), Missing::StartEmpty)
                .await
                .unwrap(),
        );
        let now = chrono::Utc::now();
        store
            .put(&Pairing::new(
                DeviceId::parse("kitchen").unwrap(),
                PublicKey::from_der(vec![1; 91]),
                PairingState::Enrolled {
                    serial: "01".to_owned(),
                    not_after: now + chrono::Duration::days(20),
                },
                now,
            ))
            .await
            .unwrap();
        let enrollment = Arc::new(Enrollment::new(
            authority,
            store,
            Arc::new(SystemClock::new(London)),
            EnrollmentPolicy {
                certificate_lifetime: chrono::Duration::days(90),
                retry_after: chrono::Duration::minutes(5),
                require_channel_binding: false,
            },
        ));
        let layouts = Layouts::new(vec![
            Layout::new(DEFAULT_LAYOUT, "Everything"),
            Layout::new("departures", "Just the trains"),
        ])
        .unwrap();
        let (base, _) = start_layouts(
            Arc::new(DirectoryImages::new(directory.to_path_buf())),
            ImageFormat::Bmp,
            Arc::new(SystemClock::new(London)),
            Some(enrollment.clone()),
            layouts,
        )
        .await;
        // The authority's directory has to outlive the test.
        std::mem::forget(pki);
        (base, enrollment)
    }

    async fn choose(enrollment: &Enrollment, layout: Option<&str>) {
        enrollment
            .set_layout(
                &DeviceId::parse("kitchen").unwrap(),
                layout.map(|name| LayoutName::parse(name).unwrap()),
            )
            .await
            .unwrap();
    }

    async fn publish_both(directory: &std::path::Path) {
        publish(directory, ImageFormat::Bmp, b"all of it")
            .await
            .unwrap();
        publish_layout(directory, "departures", ImageFormat::Bmp, b"trains")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_display_is_sent_the_layout_the_owner_chose_for_it_unless_it_asks_for_another() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, enrollment) = with_a_member(tmp.path()).await;
        publish_both(tmp.path()).await;
        let image = |query: &'static str| {
            let url = format!("{base}/image{query}");
            async move { body(url).await.1 }
        };

        assert_eq!(image("?device=kitchen").await, "all of it");
        choose(&enrollment, Some("departures")).await;

        assert_eq!(image("?device=kitchen").await, "trains");
        // What the display itself asks for wins, and a display nobody chose for gets the default.
        assert_eq!(image("?device=kitchen&layout=dashboard").await, "all of it");
        assert_eq!(image("").await, "all of it");
        assert_eq!(image("?device=hall").await, "all of it");

        choose(&enrollment, None).await;
        assert_eq!(image("?device=kitchen").await, "all of it");
    }

    #[tokio::test]
    async fn a_layout_that_has_since_gone_is_not_a_reason_to_send_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, enrollment) = with_a_member(tmp.path()).await;
        publish_both(tmp.path()).await;
        // The file of displays outlives a configuration that no longer has the layout.
        enrollment
            .set_layout(
                &DeviceId::parse("kitchen").unwrap(),
                Some(LayoutName::parse("retired").unwrap()),
            )
            .await
            .unwrap();

        assert_eq!(
            body(format!("{base}/image?device=kitchen")).await.1,
            "all of it"
        );
        assert_eq!(body(format!("{base}/plan?device=kitchen")).await.0, 200);
    }

    #[tokio::test]
    async fn a_display_given_another_layout_is_told_its_picture_changed_even_at_the_same_version() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, enrollment) = with_a_member(tmp.path()).await;
        publish_both(tmp.path()).await;
        // Rendered in the same second, as two layouts of one run can be: the same version.
        let moment = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        for file in ["image.bmp"] {
            for folder in [DEFAULT_LAYOUT, "departures"] {
                std::fs::File::options()
                    .write(true)
                    .open(tmp.path().join(folder).join(file))
                    .unwrap()
                    .set_modified(moment)
                    .unwrap();
            }
        }
        let plan = |query: String| {
            let url = format!("{base}/plan?device=kitchen{query}");
            async move { serde_json::from_str::<serde_json::Value>(&body(url).await.1).unwrap() }
        };
        let version = plan(String::new()).await["version"].as_u64().unwrap();

        // It fetches the default layout and has it.
        body(format!("{base}/image?device=kitchen")).await;
        assert_eq!(plan(format!("&have={version}")).await["changed"], false);

        // The owner gives it the other layout. Same version, but not the picture it shows.
        choose(&enrollment, Some("departures")).await;
        let told = plan(format!("&have={version}")).await;
        assert_eq!(told["version"].as_u64().unwrap(), version);
        assert_eq!(told["changed"], true);
        // Until it fetches that picture; and then it is up to date.
        assert_eq!(plan(format!("&have={version}")).await["changed"], true);
        assert_eq!(
            body(format!("{base}/image?device=kitchen")).await.1,
            "trains"
        );
        assert_eq!(plan(format!("&have={version}")).await["changed"], false);
    }

    #[tokio::test]
    async fn a_display_that_has_not_fetched_anything_is_not_told_its_picture_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, enrollment) = with_a_member(tmp.path()).await;
        publish_both(tmp.path()).await;
        choose(&enrollment, Some("departures")).await;
        let first: serde_json::Value =
            serde_json::from_str(&body(format!("{base}/plan?device=kitchen")).await.1).unwrap();
        let version = first["version"].as_u64().unwrap();

        let again: serde_json::Value = serde_json::from_str(
            &body(format!("{base}/plan?device=kitchen&have={version}"))
                .await
                .1,
        )
        .unwrap();

        // Nothing is known of what it shows, so the version is all there is to go on.
        assert_eq!(again["changed"], false);
    }
}
