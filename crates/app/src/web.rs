//! Offline-embedded web chat bundle + a tiny localhost host.
//!
//! The Ionic/React bundle is built to `web/dist/` (`npm run build` in
//! `crates/app/web/`). Files are baked into the binary at compile time.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

/// `web/dist/index.html`
pub const INDEX_HTML: &str = include_str!("../web/dist/index.html");

/// Bundled assets keyed by path relative to `web/dist/`. `None` for unknown paths.
pub fn asset(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "assets/app.js" => include_bytes!("../web/dist/assets/app.js"),
        "assets/app.css" => include_bytes!("../web/dist/assets/app.css"),
        "synapse-core.js" => include_bytes!("../web/dist/synapse-core.js"),
        "logo.svg" => include_bytes!("../web/dist/logo.svg"),
        "vendor/marked.min.js" => include_bytes!("../web/dist/vendor/marked.min.js"),
        "vendor/highlight.min.js" => include_bytes!("../web/dist/vendor/highlight.min.js"),
        "vendor/github-dark.min.css" => include_bytes!("../web/dist/vendor/github-dark.min.css"),
        "vendor/github.min.css" => include_bytes!("../web/dist/vendor/github.min.css"),
        _ => return None,
    })
}

/// MIME type for a bundled asset path.
pub fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Bind a listener on `127.0.0.1:port` (port 0 = OS-assigned ephemeral).
pub fn bind(port: u16) -> std::io::Result<(TcpListener, u16)> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let actual = listener.local_addr()?.port();
    Ok((listener, actual))
}

/// Serve the bundle on `listener` forever (blocking).
pub fn serve(listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(mut s) = stream else { continue };
        std::thread::spawn(move || handle(&mut s));
    }
}

/// Start the host on a background thread bound to an ephemeral localhost port.
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
