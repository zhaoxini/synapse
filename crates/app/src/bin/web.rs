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
//!
//! ponytail: hand-written mini HTTP server on std::net. ~80 lines, zero new
//! deps; this is a static-file server for 6 known paths, not a real web server.
//! Upgrade to axum/hyper if auth, POST, or streaming ever lands here.

use std::io::{Read, Write};
use std::net::TcpListener;

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

    let listener = TcpListener::bind(("127.0.0.1", port))
        .unwrap_or_else(|e| {
            eprintln!("could not bind 127.0.0.1:{port}: {e}");
            std::process::exit(1);
        });
    println!("synapse-web serving chat bundle on http://127.0.0.1:{port}");
    println!("open: http://127.0.0.1:{port}/?host=HOST&port=PORT&token=TOKEN");

    for stream in listener.incoming() {
        let Ok(mut s) = stream else { continue };
        let _ = std::thread::spawn(move || handle(&mut s));
    }
}

fn handle(s: &mut std::net::TcpStream) {
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut buf = [0u8; 4096];
    let n = match s.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    // First request line: `GET /path HTTP/1.1`
    let req_line = req.lines().next().unwrap_or("");
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    if method != "GET" {
        let _ = write_simple(s, 405, "text/plain", "method not allowed");
        return;
    }
    // Strip querystring.
    let path = path.split('?').next().unwrap_or("/");
    let path = match path {
        "/" | "/index.html" => "index.html",
        p if p.starts_with('/') => &p[1..],
        p => p,
    };

    if path == "index.html" {
        let _ = write_simple(s, 200, "text/html; charset=utf-8", synapse_app::web::INDEX_HTML);
        return;
    }
    match synapse_app::web::asset(path) {
        Some(bytes) => {
            let _ = write_bytes(s, 200, synapse_app::web::mime(path), bytes);
        }
        None => {
            let _ = write_simple(s, 404, "text/plain", "not found");
        }
    }
}

fn write_simple(s: &mut std::net::TcpStream, code: u16, mime: &str, body: &str) -> std::io::Result<()> {
    write_bytes(s, code, mime, body.as_bytes())
}

fn write_bytes(
    s: &mut std::net::TcpStream,
    code: u16,
    mime: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {len}\r\nConnection: close\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        len = body.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    s.flush()
}
