//! Blocking client for typesafe.ai's System One endpoint (the Jev model).
//!
//! Used by the patcher's ghost-cable suggestions (bead eseq-c049). One call
//! carries a state string and a dictionary of typed questions; every question
//! is evaluated in parallel, so a fan-out of one question per open port costs
//! about the same as a single question. Unlike the chat-model providers in
//! `network.rs` there is no conversation: the answer is structured data the
//! widget decodes itself, so this stays a thin POST.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};

pub const JEV_API_KEY_ENV: &str = "JEV_API_KEY";
const SYSTEM_ONE_URL: &str = "https://api.typesafe.ai/v1/systemone";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// The same trimmed rule the patcher's feature gate uses, so the gate and the
/// reader cannot disagree about a blank key.
pub fn api_key() -> Option<String> {
    eseqlisp::widget_render::patcher::jev_api_key()
}

/// One pooled client for every request: keeps the TLS connection warm across
/// selections instead of paying a handshake per suggestion.
fn client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("jev client: {error}"))?;
    Ok(CLIENT.get_or_init(|| client))
}

/// POST `body` (a complete `/v1/systemone` payload) and return the parsed
/// answer. Errors carry the HTTP status and the server's message so the
/// status line can show why a suggestion never arrived.
pub fn system_one(body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let key = api_key().ok_or_else(|| format!("{JEV_API_KEY_ENV} is not set"))?;
    let response = client()?
        .post(SYSTEM_ONE_URL)
        .header(AUTHORIZATION, format!("Bearer {key}"))
        .header(CONTENT_TYPE, "application/json")
        .json(body)
        .send()
        .map_err(|error| format!("jev request failed: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|error| format!("jev response unreadable: {error}"))?;
    if !status.is_success() {
        let detail = text.chars().take(400).collect::<String>();
        return Err(format!("jev HTTP {status}: {detail}"));
    }
    serde_json::from_str(&text).map_err(|error| format!("jev response is not JSON: {error}"))
}
