//! Embed an optional default relay URL at compile time.
//! Set `SYNAPSE_RELAY=wss://relay.example.com` when building release binaries.

fn main() {
    let relay = option_env!("SYNAPSE_RELAY").unwrap_or("");
    println!("cargo:rustc-env=SYNAPSE_DEFAULT_RELAY={relay}");
    println!("cargo:rerun-if-env-changed=SYNAPSE_RELAY");
}
