//! Embed default relay URL at compile time.
//! Override with `SYNAPSE_RELAY=wss://other.example.com` when building.

const DEFAULT_RELAY: &str = "wss://zx0623.duckdns.org";

fn main() {
    let relay = option_env!("SYNAPSE_RELAY").unwrap_or(DEFAULT_RELAY);
    println!("cargo:rustc-env=SYNAPSE_DEFAULT_RELAY={relay}");
    println!("cargo:rerun-if-env-changed=SYNAPSE_RELAY");
}
