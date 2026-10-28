//! Dumps the gateway model catalog: the authoritative `key` <-> `display_name`
//! mapping the user's account is entitled to.
//!
//! The CLI stores the same encrypted catalog at
//! `~/.qoder-cn/.models/<uid>/catalog-v6`. This probe decrypts that file with
//! the seam's own wasm and prints every entry, so the static mapping table can
//! be built from live truth rather than guessed.
//!
//! Run:
//!
//! ```text
//! $env:QODER_CATALOG_FILE = "C:/Users/<user>/.qoder-cn/.models/<uid>/catalog-v6"
//! cargo test --features subscription-auth --test catalog_dump -- --ignored --nocapture
//! ```

#![cfg(feature = "subscription-auth")]

use taiji_qoder_adapter::qoder_model_catalog;

#[test]
#[ignore = "reads the local CLI catalog; run with --ignored"]
fn dump_catalog_mapping() {
    let path = std::env::var("QODER_CATALOG_FILE")
        .expect("QODER_CATALOG_FILE must point at the CLI's catalog-v6 file");
    let raw = std::fs::read_to_string(&path).expect("catalog file is readable");
    // The CLI stores the gateway payload behind a `UU1DATA` file header; the
    // wasm decrypts the payload itself.
    let payload = raw.strip_prefix("UU1DATA").unwrap_or(&raw);

    // `qoder_model_catalog` decrypts through the wasm, then parses.
    match qoder_model_catalog(payload) {
        Some(models) => {
            println!("[catalog] parsed {} model ids:", models.len());
            for model in models {
                println!("[catalog]   id={model}");
            }
        }
        None => {
            println!("[catalog] seam could not parse the catalog payload");
            let head: String = raw.chars().take(80).collect();
            println!("[catalog] raw head: {head}");
        }
    }
}
