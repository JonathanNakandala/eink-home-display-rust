//! `GET /image`: the rendered picture, in the format the client asks for.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use super::{negotiate, server_error, Published};
use crate::domain::models::display::ImageFormat;

/// Serves the image in the format the client asked for with `Accept` (see `negotiate`): any it can
/// decode, the server's preferred one if it has no preference. 406 if it can decode none of them.
pub(super) async fn image(State(published): State<Arc<Published>>, headers: HeaderMap) -> Response {
    let mut available = Vec::new();
    for format in ImageFormat::ALL {
        match published.images.published_at(format).await {
            Ok(Some(_)) => available.push(format),
            Ok(None) => {}
            Err(e) => return server_error("inspect", e),
        }
    }
    if available.is_empty() {
        return (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response();
    }
    let accept = headers.get(header::ACCEPT).and_then(|value| value.to_str().ok());
    let candidates = negotiate::acceptable(accept, published.format, &available);
    if candidates.is_empty() {
        let offered: Vec<_> = available.iter().map(|format| format.content_type()).collect();
        let body = format!("No acceptable format. Available: {}", offered.join(", "));
        return (StatusCode::NOT_ACCEPTABLE, [(header::VARY, "Accept")], body).into_response();
    }
    // The file could have been replaced between the check and the read, so fall through to the next.
    for format in candidates {
        match published.images.read(format).await {
            Ok(Some(bytes)) => {
                return (
                    [
                        (header::CONTENT_TYPE, format.content_type()),
                        // The picture changes every refresh, so nothing may reuse an old one.
                        (header::CACHE_CONTROL, "no-store"),
                        // The reply depends on the request's Accept, which a cache must know.
                        (header::VARY, "Accept"),
                    ],
                    bytes,
                )
                    .into_response();
            }
            Ok(None) => continue,
            Err(e) => return server_error("read", e),
        }
    }
    (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response()
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
}
