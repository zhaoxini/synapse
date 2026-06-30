//! `synapse-web` — offline host for the embedded chat web bundle.
//!
//! Serves the chat web app (`crates/app/web`) over a localhost port so any
//! browser or webview can load it. The web app is a normal WS client; it dials
//! the Synapse server itself. This replaces the broken Slint chat surface on
//! desktop and gives iOS a single URL to load in a WKWebView.
//!
//! Usage:
//!   synapse-web [--port 8765]
//!   then open: http://localhost:8765/?host=127.0.0.1&port=4173&token=CODE
//!
//! The `host/port/token/tls/path` query params are forwarded into the page so
//! the web app can connect without a JS<->native bridge. A native iOS host can
//! instead inject `window.__SYNAPSE__` and skip the querystring.

fn main() {
    let mut port: u16 = 8765;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--port" {
            if let Some(p) = args.next().and_then(|s| s.parse().ok()) {
                port = p;
            }
        }
    }

    let (listener, port) = synapse_app::web::bind(port).unwrap_or_else(|e| {
        eprintln!("could not bind 127.0.0.1:{port}: {e}");
        std::process::exit(1);
    });
    println!("synapse-web serving chat bundle on http://127.0.0.1:{port}");
    println!("open: http://127.0.0.1:{port}/?host=HOST&port=PORT&token=TOKEN");
    synapse_app::web::serve(listener);
}
