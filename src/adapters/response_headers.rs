//! Headers every answer of the image and EST servers carries, whatever the route or the status.
//!
//! The displays ignore them. They are for the case where a browser is pointed at the server, or a page
//! open in one on the owner's network is made to fetch from it: they stop a reply being read as something
//! it is not, shown in a frame, or shared with other sites.
//!
//! A header a handler sets itself is left as it set it.

use axum::Router;
use axum::http::{HeaderName, HeaderValue};
use tower_http::set_header::SetResponseHeaderLayer;

/// Every header added, and its value.
pub const HEADERS: [(&str, &str); 4] = [
    // The body is the type it says (`text/plain` and JSON here), and is never guessed to be HTML or a script.
    ("x-content-type-options", "nosniff"),
    // No address of this server goes out in a `Referer` to wherever a page links on to.
    ("referrer-policy", "no-referrer"),
    // Another site can't embed the reply (an `<img>` or a `<script>` pointing at it). Same-origin only.
    ("cross-origin-resource-policy", "same-origin"),
    // Not shown inside a frame on another page. Nothing else is restricted: a picture opened by itself must
    // still show, which a `default-src` rule can stop it doing.
    ("content-security-policy", "frame-ancestors 'none'"),
];

/// `router` with those headers on everything it answers, including 404s and 405s.
pub fn hardened<S: Clone + Send + Sync + 'static>(router: Router<S>) -> Router<S> {
    HEADERS.into_iter().fold(router, |router, (name, value)| {
        router.layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        ))
    })
}

#[cfg(test)]
mod tests {
    use axum::routing::get;

    use super::*;

    #[test]
    fn the_names_and_values_are_ones_http_accepts() {
        // `from_static` panics on a bad one; this makes that a test and not a failure at start-up.
        let _ = hardened(Router::<()>::new());
    }

    #[tokio::test]
    async fn a_header_a_handler_sets_is_not_replaced() {
        let app = hardened(Router::new().route(
            "/",
            get(|| async { ([("referrer-policy", "same-origin")], "x") }),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        let response = reqwest::get(format!("http://{address}/")).await.unwrap();
        assert_eq!(response.headers()["referrer-policy"], "same-origin");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        // And on a path that does not exist.
        let missing = reqwest::get(format!("http://{address}/nope"))
            .await
            .unwrap();
        assert_eq!(missing.status(), 404);
        assert_eq!(missing.headers()["x-content-type-options"], "nosniff");
    }
}
