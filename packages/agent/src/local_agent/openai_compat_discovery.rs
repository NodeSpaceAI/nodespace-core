//! Model discovery for OpenAI-compatible endpoints via `GET {base_url}/models`.
//!
//! Every OpenAI-compatible server — Ollama's `/v1`, LM Studio, vLLM, the real
//! OpenAI API — exposes a `/models` listing as part of the protocol. Querying it
//! lets a single user-configured endpoint surface all of its models in the
//! selector instead of forcing one hand-written config per model.
//!
//! The listing is deliberately thin. `/v1/models` returns only `id`, `created`
//! and `owned_by`, so a discovered row carries no size, quantization, context
//! window or capability flags. Those fields are left at their "unknown"
//! sentinels rather than guessed at.

use crate::agent_types::{ModelBackend, ModelError, ModelFamily, ModelInfo, ModelStatus};
use serde::Deserialize;
use std::time::Duration;

/// Time allowed for a discovery request, connect through response body.
///
/// Short by design: discovery runs on the catalog-listing path, which the model
/// selector awaits before it can render. A slow or unreachable endpoint must
/// degrade to "no models" quickly rather than stall the whole list.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// A single entry from a `/models` response.
#[derive(Debug, Deserialize)]
struct OpenAiModelEntry {
    id: String,
}

/// The `GET /models` response envelope.
#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelEntry>,
}

/// Build the model-listing URL for an endpoint base URL.
///
/// Tolerates a trailing slash on the configured base URL, which users routinely
/// include (`http://localhost:11434/v1/`).
fn models_url(base_url: &str) -> String {
    format!("{}/models", base_url.trim_end_matches('/'))
}

/// Parse a `/models` response body into model identifiers, preserving order.
///
/// Split out from the HTTP call so the wire-format handling is testable without
/// a live server.
fn parse_models_response(body: &str) -> Result<Vec<String>, serde_json::Error> {
    let parsed: OpenAiModelsResponse = serde_json::from_str(body)?;
    Ok(parsed.data.into_iter().map(|m| m.id).collect())
}

/// Query an OpenAI-compatible endpoint for the models it serves.
///
/// Returns the wire-protocol model identifiers, in the order the server listed
/// them. `api_key` is sent as a bearer token when non-empty; local servers
/// (Ollama, LM Studio) generally require no auth.
///
/// # Errors
///
/// Returns [`ModelError`] if the endpoint is unreachable, returns a non-success
/// status, or sends a body that is not a valid `/models` response. Callers that
/// aggregate multiple endpoints into one catalog should treat an error as "this
/// endpoint contributed nothing" rather than failing the whole listing — see
/// [`discover_models_or_empty`].
pub async fn discover_models(base_url: &str, api_key: &str) -> Result<Vec<String>, ModelError> {
    let client = reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .build()
        .map_err(|e| ModelError::Other(anyhow::anyhow!("failed to build HTTP client: {e}")))?;

    let mut request = client.get(models_url(base_url));
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }

    let response = request
        .send()
        .await
        .map_err(|e| ModelError::Other(anyhow::anyhow!("failed to reach {base_url}: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        return Err(ModelError::Other(anyhow::anyhow!(
            "{base_url} returned {status} for /models"
        )));
    }

    let body = response
        .text()
        .await
        .map_err(|e| ModelError::Other(anyhow::anyhow!("failed to read /models response: {e}")))?;

    parse_models_response(&body).map_err(|e| {
        ModelError::Other(anyhow::anyhow!(
            "malformed /models response from {base_url}: {e}"
        ))
    })
}

/// Discover models, degrading to an empty list when the endpoint cannot be
/// reached or answers with something unusable.
///
/// The catalog aggregates every configured endpoint, so one misconfigured or
/// offline provider must not blank out the others — the same reason the former
/// Ollama manager returned an empty list instead of an error when its daemon
/// was down.
pub async fn discover_models_or_empty(base_url: &str, api_key: &str) -> Vec<String> {
    match discover_models(base_url, api_key).await {
        Ok(models) => models,
        Err(e) => {
            tracing::debug!(base_url = %base_url, error = %e, "OpenAI-compat model discovery failed");
            Vec::new()
        }
    }
}

/// Build a catalog row for a discovered model.
///
/// `config_id` is the UUID of the `[[openai_compat.configs]]` entry the model
/// was discovered through, and `config_name` its cosmetic label; together with
/// the model identifier they form the addressable id
/// `openai-compat:<config_id>:<model>`.
///
/// Fields that `/models` does not report are left at their unknown sentinels:
/// zero size, empty quantization, zero minimum memory. `status` is
/// [`ModelStatus::Ready`] because a remotely-served model needs no local
/// download step.
pub fn discovered_model_info(config_id: &str, config_name: &str, model: &str) -> ModelInfo {
    ModelInfo {
        id: format!(
            "{}{}:{}",
            crate::local_agent::openai_compat_inference::OPENAI_COMPAT_PREFIX,
            config_id,
            model
        ),
        family: ModelFamily::OpenAiCompat,
        name: format!("{config_name} — {model}"),
        filename: None,
        size_bytes: 0,
        quantization: String::new(),
        url: None,
        sha256: None,
        backend: ModelBackend::OpenAiCompat,
        status: ModelStatus::Ready,
        min_memory_gb: 0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_url_appends_path() {
        assert_eq!(
            models_url("http://localhost:11434/v1"),
            "http://localhost:11434/v1/models"
        );
    }

    #[test]
    fn models_url_tolerates_trailing_slash() {
        // Users routinely paste a base URL with a trailing slash; without the
        // trim this would request "/v1//models".
        assert_eq!(
            models_url("http://localhost:11434/v1/"),
            "http://localhost:11434/v1/models"
        );
    }

    #[test]
    fn parses_real_ollama_models_response() {
        // Body shape from Ollama 0.32.1 at /v1/models — note it carries only
        // id/object/created/owned_by, no size or context length.
        //
        // The ids are incidental to what this asserts (that every `id` is
        // extracted, in order).
        let body = r#"{"object":"list","data":[
            {"id":"llama3.1:8b","object":"model","created":1783255193,"owned_by":"library"},
            {"id":"llama3.2:3b","object":"model","created":1783252404,"owned_by":"library"},
            {"id":"gemma4:e4b","object":"model","created":1775681349,"owned_by":"library"}
        ]}"#;

        let models = parse_models_response(body).expect("should parse");
        assert_eq!(models, vec!["llama3.1:8b", "llama3.2:3b", "gemma4:e4b"]);
    }

    #[test]
    fn parses_empty_model_list() {
        let models = parse_models_response(r#"{"object":"list","data":[]}"#).expect("should parse");
        assert!(models.is_empty());
    }

    #[test]
    fn parse_rejects_non_models_body() {
        // An endpoint answering 200 with unrelated JSON must not yield models.
        assert!(parse_models_response(r#"{"error":"not found"}"#).is_err());
    }

    #[test]
    fn discovered_id_round_trips_through_the_prefix_helpers() {
        use crate::local_agent::openai_compat_inference::{
            is_openai_compat, strip_openai_compat_prefix,
        };

        let info = discovered_model_info("abc-123", "Local Ollama", "llama3.1:8b");

        assert_eq!(info.id, "openai-compat:abc-123:llama3.1:8b");
        assert!(is_openai_compat(&info.id));

        // The model name itself contains a colon, so the config id must be
        // taken as the segment up to the FIRST colon — not the last, and not
        // the whole remainder.
        let rest = strip_openai_compat_prefix(&info.id);
        let (config_id, model) = rest.split_once(':').expect("id carries a model segment");
        assert_eq!(config_id, "abc-123");
        assert_eq!(model, "llama3.1:8b");
    }

    #[test]
    fn discovered_row_leaves_unreported_fields_at_unknown_sentinels() {
        let info = discovered_model_info("abc-123", "Local Ollama", "llama3.1:8b");

        // /models reports none of these; they must not be invented.
        assert_eq!(info.size_bytes, 0);
        assert_eq!(info.min_memory_gb, 0);
        assert!(info.quantization.is_empty());
        assert!(info.filename.is_none());
        assert!(info.url.is_none());
        assert!(info.sha256.is_none());

        // Remotely served: no local download step to perform.
        assert_eq!(info.status, ModelStatus::Ready);
        assert_eq!(info.backend, ModelBackend::OpenAiCompat);
    }
}
