//! `GET /image`: the rendered picture, in the format the client asks for.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use super::identity::Caller;
use super::{negotiate, transfer, Published};
use crate::domain::models::display::ImageFormat;

/// Serves the image in the format the client asked for with `Accept` (see `negotiate`): any it can
/// decode, the server's preferred one if it has no preference. 406 if it can decode none of them.
///
/// A format whose file can't be inspected or read is skipped, so one bad file doesn't take the
/// others down with it. It is a 500 only when that leaves the client nothing it accepts, and never
/// a 404 or 406: those would say the image isn't there or isn't wanted, when the server failed.
pub(super) async fn image(
    State(published): State<Arc<Published>>,
    Caller(caller): Caller,
    headers: HeaderMap,
) -> Response {
    let mut available = Vec::new();
    let mut unreadable = Vec::new();
    for format in ImageFormat::ALL {
        match published.images.published_at(format).await {
            Ok(Some(_)) => available.push(format),
            Ok(None) => {}
            Err(e) => {
                log::warn!("Skipping the {} image: failed to inspect it: {e:#}", format.extension());
                unreadable.push(format);
            }
        }
    }
    let accept = headers.get(header::ACCEPT).and_then(|value| value.to_str().ok());
    let candidates = negotiate::acceptable(accept, published.format, &available);
    if candidates.is_empty() {
        let wanted_but_broken = !negotiate::acceptable(accept, published.format, &unreadable).is_empty();
        if wanted_but_broken || (available.is_empty() && !unreadable.is_empty()) {
            return failed("The image could not be read");
        }
        if available.is_empty() {
            return (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response();
        }
        let offered: Vec<_> = available.iter().map(|format| format.content_type()).collect();
        let body = format!("No acceptable format. Available: {}", offered.join(", "));
        return (StatusCode::NOT_ACCEPTABLE, [(header::VARY, "Accept")], body).into_response();
    }
    // The file could have been replaced between the check and the read, or fail to read: either way
    // fall through to the next format the client accepts.
    let mut read_failed = false;
    for format in candidates {
        match published.images.read(format).await {
            Ok(Some(bytes)) => {
                // Which display got which format, so a comparison of formats says who it was run on. Noted as the
                // reply is handed to the server, not when the last byte has gone: that is for the transfer timing.
                if let Some(device) = &caller {
                    published.handles.devices.image_served(device, format, bytes.len() as u64, published.clock.now());
                }
                let length = bytes.len();
                return (
                    [
                        (header::CONTENT_TYPE, format.content_type()),
                        // Sent as a stream so a dropped download is noticed, which loses the length otherwise.
                        (header::CONTENT_LENGTH, length.to_string().as_str()),
                        // The picture changes every refresh, so nothing may reuse an old one.
                        (header::CACHE_CONTROL, "no-store"),
                        // The reply depends on the request's Accept, which a cache must know.
                        (header::VARY, "Accept"),
                    ],
                    transfer::watched(bytes, caller, format),
                )
                    .into_response();
            }
            Ok(None) => continue,
            Err(e) => {
                log::warn!("Skipping the {} image: failed to read it: {e:#}", format.extension());
                read_failed = true;
            }
        }
    }
    if read_failed {
        return failed("The image could not be read");
    }
    (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response()
}

/// A server error that says what failed, without the detail (which is in the log).
fn failed(message: &'static str) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
}

#[cfg(test)]
mod tests {
    use super::super::testing::{publish, start};
    use super::*;

    #[tokio::test]
    async fn serves_the_published_image_with_its_content_type() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;

        assert_eq!(reqwest::get(format!("{base}/image")).await.unwrap().status(), 404);

        publish(tmp.path(), ImageFormat::Bmp, &[1, 2, 3]).await.unwrap();
        let response = reqwest::get(format!("{base}/image")).await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "image/bmp");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.bytes().await.unwrap().as_ref(), [1, 2, 3]);

        // A new publish replaces it and leaves no staging file behind.
        publish(tmp.path(), ImageFormat::Bmp, &[9]).await.unwrap();
        let response = reqwest::get(format!("{base}/image")).await.unwrap();
        assert_eq!(response.bytes().await.unwrap().as_ref(), [9]);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    async fn fetch(base: &str, accept: Option<&str>) -> reqwest::Response {
        let request = reqwest::Client::new().get(format!("{base}/image"));
        match accept {
            Some(accept) => request.header("Accept", accept),
            None => request,
        }
        .send()
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn image_follows_the_accept_header() {
        let tmp = tempfile::tempdir().unwrap();
        // The server prefers PNG, and has both.
        let base = start(tmp.path().to_path_buf(), ImageFormat::Png).await;
        publish(tmp.path(), ImageFormat::Bmp, b"bmp-bytes").await.unwrap();
        publish(tmp.path(), ImageFormat::Png, b"png-bytes").await.unwrap();

        for (accept, content_type, body) in [
            (None, "image/png", "png-bytes"),
            (Some("*/*"), "image/png", "png-bytes"),
            (Some("image/bmp"), "image/bmp", "bmp-bytes"),
            (Some("image/png"), "image/png", "png-bytes"),
            (Some("image/bmp, image/png;q=0.5"), "image/bmp", "bmp-bytes"),
            // What ESPHome's format AUTO sends: no preference, so the server's wins.
            (Some("image/*,*/*;q=0.8"), "image/png", "png-bytes"),
        ] {
            let response = fetch(&base, accept).await;
            assert_eq!(response.status(), 200, "{accept:?}");
            assert_eq!(response.headers()["content-type"], content_type, "{accept:?}");
            assert_eq!(response.headers()["vary"], "Accept", "{accept:?}");
            assert_eq!(response.headers()["cache-control"], "no-store", "{accept:?}");
            assert_eq!(response.text().await.unwrap(), body, "{accept:?}");
        }
    }

    #[tokio::test]
    async fn two_displays_fetching_at_once_are_each_recorded_with_what_they_were_sent() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Png).await;
        publish(tmp.path(), ImageFormat::Bmp, b"bmp-bytes").await.unwrap();
        publish(tmp.path(), ImageFormat::Png, b"png-bytes").await.unwrap();
        for name in ["kitchen", "hall"] {
            assert_eq!(reqwest::get(format!("{base}/plan?device={name}")).await.unwrap().status(), 200);
        }

        let fetch_as = |name: &'static str, accept: &'static str| {
            let url = format!("{base}/image?device={name}");
            async move { reqwest::Client::new().get(url).header("Accept", accept).send().await.unwrap().text().await.unwrap() }
        };
        let (kitchen, hall) = tokio::join!(fetch_as("kitchen", "image/bmp"), fetch_as("hall", "image/png"));
        assert_eq!((kitchen.as_str(), hall.as_str()), ("bmp-bytes", "png-bytes"));

        let status = reqwest::get(format!("{base}/status")).await.unwrap().json::<serde_json::Value>().await.unwrap();
        let sent = |name: &str| {
            let device = status["devices"].as_array().unwrap().iter().find(|d| d["name"] == name).unwrap().clone();
            (device["last_image"]["format"].clone(), device["last_image"]["bytes"].clone())
        };
        assert_eq!(sent("kitchen"), (serde_json::json!("bmp"), serde_json::json!(9)));
        assert_eq!(sent("hall"), (serde_json::json!("png"), serde_json::json!(9)));
    }

    #[tokio::test]
    async fn a_missing_or_bad_name_is_still_served_and_just_not_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"x").await.unwrap();
        // Checked in under a good name, so a record exists that a wrongly attributed fetch could have changed.
        reqwest::get(format!("{base}/plan?device=kitchen")).await.unwrap();

        for query in ["", "?device=", "?device=has%20space", "?device=stranger", "?device=%22%0A"] {
            let response = reqwest::get(format!("{base}/image{query}")).await.unwrap();
            assert_eq!(response.status(), 200, "{query}");
        }
        let status = reqwest::get(format!("{base}/status")).await.unwrap().json::<serde_json::Value>().await.unwrap();
        let devices = status["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 1, "a fetch alone adds no display: {devices:?}");
        assert!(devices[0]["last_image"].is_null(), "{devices:?}");
    }

    #[tokio::test]
    async fn an_unacceptable_accept_header_is_a_406_that_lists_what_there_is() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"x").await.unwrap();
        publish(tmp.path(), ImageFormat::Png, b"y").await.unwrap();

        for accept in ["image/gif", "text/html", "image/bmp;q=0, image/png;q=0"] {
            let response = fetch(&base, Some(accept)).await;
            assert_eq!(response.status(), 406, "{accept}");
            assert_eq!(response.headers()["vary"], "Accept");
            let body = response.text().await.unwrap();
            assert!(body.contains("image/bmp") && body.contains("image/png"), "{body}");
        }
    }

    #[tokio::test]
    async fn only_the_formats_that_exist_are_offered() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Png).await;
        assert_eq!(fetch(&base, None).await.status(), 404);

        // Only a BMP exists, though PNG is preferred: wildcards get it, and asking for PNG is a 406.
        publish(tmp.path(), ImageFormat::Bmp, b"bmp-bytes").await.unwrap();
        let response = fetch(&base, Some("*/*")).await;
        assert_eq!(response.headers()["content-type"], "image/bmp");
        let refused = fetch(&base, Some("image/png")).await;
        assert_eq!(refused.status(), 406);
        assert!(refused.text().await.unwrap().ends_with("image/bmp"));
    }

    /// A server preferring PNG over both formats, one of which misbehaves.
    async fn start_faulty(
        directory: &std::path::Path,
        cannot_inspect: Option<ImageFormat>,
        cannot_read: Option<ImageFormat>,
    ) -> String {
        use super::super::testing::{start_with, Faulty};
        use crate::adapters::clock::SystemClock;
        use crate::adapters::published_images::DirectoryImages;

        publish(directory, ImageFormat::Bmp, b"bmp-bytes").await.unwrap();
        publish(directory, ImageFormat::Png, b"png-bytes").await.unwrap();
        let images = Faulty { inner: DirectoryImages::new(directory), cannot_inspect, cannot_read };
        start_with(Arc::new(images), ImageFormat::Png, Arc::new(SystemClock)).await.0
    }

    #[tokio::test]
    async fn a_format_that_cannot_be_inspected_does_not_take_the_others_down() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start_faulty(tmp.path(), Some(ImageFormat::Png), None).await;

        // The server prefers PNG, but it is the broken one: anyone who accepts BMP still gets it.
        for accept in [None, Some("*/*"), Some("image/bmp"), Some("image/png, image/bmp;q=0.5")] {
            let response = fetch(&base, accept).await;
            assert_eq!(response.status(), 200, "{accept:?}");
            assert_eq!(response.headers()["content-type"], "image/bmp", "{accept:?}");
        }
        // Someone who can only decode PNG is let down by the server, not by a mismatch.
        let response = fetch(&base, Some("image/png")).await;
        assert_eq!(response.status(), 500);
    }

    #[tokio::test]
    async fn a_format_that_cannot_be_read_falls_through_to_the_next() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start_faulty(tmp.path(), None, Some(ImageFormat::Png)).await;

        let response = fetch(&base, Some("image/png, image/bmp;q=0.5")).await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "image/bmp");
        assert_eq!(response.text().await.unwrap(), "bmp-bytes");
        assert_eq!(fetch(&base, Some("image/png")).await.status(), 500);
    }

    #[tokio::test]
    async fn when_nothing_can_be_read_it_is_a_server_error_not_a_missing_image() {
        let tmp = tempfile::tempdir().unwrap();
        publish(tmp.path(), ImageFormat::Bmp, b"x").await.unwrap();
        for (inspect, read) in [(Some(ImageFormat::Bmp), None), (None, Some(ImageFormat::Bmp))] {
            use super::super::testing::{start_with, Faulty};
            use crate::adapters::clock::SystemClock;
            use crate::adapters::published_images::DirectoryImages;

            let images = Faulty { inner: DirectoryImages::new(tmp.path()), cannot_inspect: inspect, cannot_read: read };
            let (base, _) = start_with(Arc::new(images), ImageFormat::Bmp, Arc::new(SystemClock)).await;
            let response = fetch(&base, None).await;
            assert_eq!(response.status(), 500, "{inspect:?} {read:?}");
            assert_eq!(response.text().await.unwrap(), "The image could not be read");
        }
    }
}
