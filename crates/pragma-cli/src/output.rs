//! Printing: human text by default, JSON for the app with `--json`.

use anyhow::Result;
use pragma_engine::lock::LOCK_FORMAT;
use serde::Serialize;

use crate::json::VersionOutput;

pub fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn version(json: bool) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    if json {
        print_json(&VersionOutput {
            version: version.to_string(),
            lock_format: LOCK_FORMAT,
        })
    } else {
        println!("pragma {version} (lock format {LOCK_FORMAT})");
        Ok(())
    }
}
