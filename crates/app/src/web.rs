//! Offline-embedded web chat bundle.
//!
//! The entire post-pairing experience (chat, composer, drawer, sessions) runs
//! in a webview against this bundle. Files are baked into the binary at compile
//! time so there is zero network dependency and instant load. A native host
//! (iOS WKWebView, or a desktop webview) loads [`INDEX_HTML`] with a base URL
//! that resolves the `vendor/` and `app.*` relative paths via [`asset`].
//!
//! Asset names are the path relative to `web/` (e.g. `vendor/marked.min.js`,
//! `app.css`). Unknown names return `None`.

/// `web/index.html`
pub const INDEX_HTML: &str = include_str!("../web/index.html");

/// All bundled assets keyed by their path relative to `web/`.
/// Returns the raw bytes, or `None` for unknown paths.
pub fn asset(path: &str) -> Option<&'static [u8]> {
    // ponytail: explicit match over a build-time directory walk — paths are
    // fixed and few, and it keeps the public surface auditable.
    Some(match path {
        "app.css" => include_bytes!("../web/app.css"),
        "app.js" => include_bytes!("../web/app.js"),
        "vendor/marked.min.js" => include_bytes!("../web/vendor/marked.min.js"),
        "vendor/highlight.min.js" => include_bytes!("../web/vendor/highlight.min.js"),
        "vendor/github-dark.min.css" => include_bytes!("../web/vendor/github-dark.min.css"),
        _ => return None,
    })
}

/// MIME type for a bundled asset path.
pub fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        _ => "application/octet-stream",
    }
}
