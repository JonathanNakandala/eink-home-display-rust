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
    /// The layout asked for, or the default when none was; or the answer to give when there is no such layout.
    /// A name that isn't one is the same as a name nothing has: not found, and what is.
    pub(super) fn resolve<'a>(
        &self,
        published: &'a Published,
    ) -> Result<&'a Layout, (StatusCode, String)> {
        let Some(name) = self.layout.as_deref() else {
            return Ok(published.layouts.default_layout());
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
}
