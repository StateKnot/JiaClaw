// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Embedded, same-origin management UI without external assets.
use axum::{
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
};
fn asset(content_type: &'static str, body: &'static str) -> Response {
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("content-security-policy", HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"));
    response
}
pub(super) async fn index() -> Response {
    asset("text/html; charset=utf-8", include_str!("../ui/index.html"))
}
pub(super) async fn javascript() -> Response {
    asset(
        "text/javascript; charset=utf-8",
        include_str!("../ui/app.js"),
    )
}
pub(super) async fn stylesheet() -> Response {
    asset("text/css; charset=utf-8", include_str!("../ui/app.css"))
}
pub(super) async fn turn_stream() -> Response {
    asset(
        "text/javascript; charset=utf-8",
        include_str!("../ui/turn-stream.js"),
    )
}
