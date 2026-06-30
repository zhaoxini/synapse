fn main() {
    slint_build::compile("ui/app.slint").unwrap();

    // The web chat bundle is baked into the binary via include_str!/include_bytes!
    // in src/web.rs. Those assets are NOT .rs files, so editing them did not always
    // invalidate the crate's fingerprint — which shipped a STALE bundle to iOS
    // (the Xcode run-script reused a cached .a). Declare each embedded asset a build
    // input so any bundle change forces a recompile. Keep in sync with web::asset().
    for f in [
        "web/index.html",
        "web/app.css",
        "web/app.js",
        "web/vendor/marked.min.js",
        "web/vendor/highlight.min.js",
        "web/vendor/github-dark.min.css",
        "web/vendor/github.min.css",
    ] {
        println!("cargo:rerun-if-changed={f}");
    }
}
