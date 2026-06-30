//! Offline-embedded web chat bundle + a tiny localhost host.
//!
//! The entire post-pairing experience (chat, composer, drawer, sessions) runs
//! in a webview against this bundle. Files are baked into the binary at compile
//! time so there is zero network dependency and instant load. A native host
//! (iOS WKWebView, or a desktop browser) loads the served `index.html`; the web
//! app is a normal WS client that dials the Synapse server itself.
//!
//! Asset names are the path relative to `web/` (e.g. `vendor/marked.min.js`,
//! `app.css`). Unknown names return `None`.
//!
//! ponytail: hand-written mini HTTP server on std::net. ~90 lines, zero new
//! deps; a static-file server for 6 known paths, not a real web server.
//! Upgrade to axum/hyper if auth, POST, or streaming ever lands here.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

/// `web/index.html`
pub const INDEX_HTML: &str = include_str!("../web/index.html");

/// Bundled assets keyed by path relative to `web/`. `None` for unknown paths.
pub fn asset(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "app.css" => include_bytes!("../web/app.css"),
        "app.js" => include_bytes!("../web/app.js"),
        "vendor/marked.min.js" => include_bytes!("../web/vendor/marked.min.js"),
        "vendor/highlight.min.js" => include_bytes!("../web/vendor/highlight.min.js"),
        "vendor/github-dark.min.css" => include_bytes!("../web/vendor/github-dark.min.css"),
        "vendor/github.min.css" => include_bytes!("../web/vendor/github.min.css"),
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

/// Bind a listener on `127.0.0.1:port` (port 0 = OS-assigned ephemeral).
/// Returns the listener and the actual bound port.
pub fn bind(port: u16) -> std::io::Result<(TcpListener, u16)> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let actual = listener.local_addr()?.port();
    Ok((listener, actual))
}

/// Serve the bundle on `listener` forever (blocking). Each connection is
/// handled on its own thread. Used by the `synapse-web` bin.
pub fn serve(listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(mut s) = stream else { continue };
        std::thread::spawn(move || handle(&mut s));
    }
}

/// Start the host on a background thread bound to an ephemeral localhost port.
/// Returns the chosen port. Used by the iOS WKWebView host.
pub fn spawn_host() -> std::io::Result<u16> {
    let (listener, port) = bind(0)?;
    std::thread::Builder::new()
        .name("synapse-web-host".into())
        .spawn(move || serve(listener))?;
    Ok(port)
}

fn handle(s: &mut TcpStream) {
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut buf = [0u8; 4096];
    let n = match s.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    let req_line = req.lines().next().unwrap_or("");
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    if method != "GET" {
        let _ = write_bytes(s, 405, "text/plain", b"method not allowed");
        return;
    }
    let path = path.split('?').next().unwrap_or("/");
    let path = match path {
        "/" | "/index.html" => "index.html",
        p if p.starts_with('/') => &p[1..],
        p => p,
    };
    if path == "index.html" {
        let _ = write_bytes(s, 200, "text/html; charset=utf-8", INDEX_HTML.as_bytes());
        return;
    }
    match asset(path) {
        Some(bytes) => {
            let _ = write_bytes(s, 200, mime(path), bytes);
        }
        None => {
            let _ = write_bytes(s, 404, "text/plain", b"not found");
        }
    }
}

fn write_bytes(s: &mut TcpStream, code: u16, mime: &str, body: &[u8]) -> std::io::Result<()> {
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
