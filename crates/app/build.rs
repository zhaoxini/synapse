fn main() {
    slint_build::compile("ui/app.slint").unwrap();

    // Ionic web bundle: built to web/dist/ via `npm run build`. Track sources so
    // edits invalidate; CI/local must run the build before `cargo build`.
    println!("cargo:rerun-if-changed=web/dist/index.html");
    println!("cargo:rerun-if-changed=web/dist/assets/app.js");
    println!("cargo:rerun-if-changed=web/dist/assets/app.css");
    println!("cargo:rerun-if-changed=web/dist/synapse-core.js");
    for f in [
        "web/index.html",
        "web/vite.config.ts",
        "web/src/main.tsx",
        "web/src/App.tsx",
        "web/public/synapse-core.js",
        "web/src/theme/synapse.css",
        "web/src/theme/variables.css",
        "web/public/logo.svg",
    ] {
        println!("cargo:rerun-if-changed={f}");
    }
}
