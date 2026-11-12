//! Print the API's `OpenAPI` description as JSON (`just api-spec` writes it to
//! `backend/openapi.json`).
//!
//! Built from the route tree alone, so it needs no database, environment or
//! adapters. An example rather than a `[[bin]]`, so release image builds don't
//! compile it.

use std::io::Write;

fn main() -> std::io::Result<()> {
    let spec = overkill_todo::infrastructure::api::openapi::openapi();
    let json = spec.to_pretty_json().map_err(std::io::Error::other)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(json.as_bytes())?;
    stdout.write_all(b"\n")
}
