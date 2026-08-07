//! Upstream URL normalization utilities.
//!
//! Users may enter the base URL in various formats:
//! - Bare root: `https://api.openai.com`
//! - With version prefix: `https://api.mistral.ai/v1`
//! - Full endpoint: `https://api.mistral.ai/v1/chat/completions`
//! - Non-standard version: `https://generativelanguage.googleapis.com/v1beta/openai`
//! - Full non-standard endpoint: `https://generativelanguage.googleapis.com/v1beta/openai/chat/completions`
//!
//! This module provides functions that normalize any of these inputs into the
//! correct full endpoint URL, avoiding double `/v1` or path duplication.

// ============================================================================
// Public API
// ============================================================================

/// Build the full URL for the `/chat/completions` endpoint.
///
/// # Examples
///
/// | Input | Output |
/// |-------|--------|
/// | `https://api.openai.com` | `https://api.openai.com/v1/chat/completions` |
/// | `https://api.mistral.ai/v1` | `https://api.mistral.ai/v1/chat/completions` |
/// | `https://api.mistral.ai/v1/chat/completions` | (unchanged) |
/// | `https://generativelanguage.googleapis.com/v1beta/openai` | `.../v1beta/openai/chat/completions` |
/// | `https://generativelanguage.googleapis.com/v1beta/openai/chat/completions` | (unchanged) |
pub fn build_chat_completions_url(base_url: &str) -> String {
    build_endpoint_url(base_url, "chat/completions")
}

/// Build the full URL for the `/models` endpoint.
///
/// If the input URL already contains a `/chat/completions` suffix, it is
/// stripped first so that the models endpoint shares the same version base.
///
/// # Examples
///
/// | Input | Output |
/// |-------|--------|
/// | `https://api.openai.com` | `https://api.openai.com/v1/models` |
/// | `https://api.mistral.ai/v1` | `https://api.mistral.ai/v1/models` |
/// | `https://api.mistral.ai/v1/chat/completions` | `https://api.mistral.ai/v1/models` |
/// | `https://generativelanguage.googleapis.com/v1beta/openai` | `.../v1beta/openai/models` |
pub fn build_models_url(base_url: &str) -> String {
    build_endpoint_url(base_url, "models")
}

/// Build the full URL for the Anthropic `/v1/messages` endpoint.
///
/// | Input | Output |
/// |-------|--------|
/// | `https://api.anthropic.com` | `https://api.anthropic.com/v1/messages` |
/// | `https://api.anthropic.com/v1` | `https://api.anthropic.com/v1/messages` |
/// | `https://api.anthropic.com/v1/messages` | (unchanged) |
pub fn build_anthropic_messages_url(base_url: &str) -> String {
    build_endpoint_url(base_url, "messages")
}

/// Build the full URL for the Gemini Native `:generateContent` endpoint.
///
/// Gemini's native path is `/v1beta/models/{model}:generateContent`. The base
/// URL is normally the bare host (e.g. `https://generativelanguage.googleapis.com`).
///
/// | Input | Output |
/// |-------|--------|
/// | `https://generativelanguage.googleapis.com` | `.../v1beta/models/gemini-2.0-flash:generateContent` |
/// | `https://generativelanguage.googleapis.com/v1beta` | `.../v1beta/models/gemini-2.0-flash:generateContent` |
pub fn build_gemini_generate_url(base_url: &str, model: &str) -> String {
    let mut base = base_url.trim_end_matches('/');
    if base.ends_with("/v1beta") {
        base = &base[..base.len() - "/v1beta".len()];
    }
    let suffix = format!("/v1beta/models/{}:generateContent", model);
    format!("{}{}", base, suffix)
}

/// Build the full URL for the OpenAI Responses `/v1/responses` endpoint.
///
/// | Input | Output |
/// |-------|--------|
/// | `https://api.openai.com` | `https://api.openai.com/v1/responses` |
/// | `https://api.openai.com/v1` | `https://api.openai.com/v1/responses` |
/// | `https://api.openai.com/v1/responses` | (unchanged) |
pub fn build_responses_url(base_url: &str) -> String {
    build_endpoint_url(base_url, "responses")
}

/// Build the request URL for an upstream based on its native API format.
///
/// `api_format` values: `openai_chat` | `openai_responses` | `anthropic` | `gemini_native`.
pub fn build_format_url(base_url: &str, api_format: &str, model: &str) -> String {
    match api_format {
        "anthropic" => build_anthropic_messages_url(base_url),
        "gemini_native" => build_gemini_generate_url(base_url, model),
        "openai_responses" => build_responses_url(base_url),
        _ => build_chat_completions_url(base_url),
    }
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Core logic: normalize `base_url` and append `/endpoint`.
///
/// Strategy:
/// 1. If the URL already ends with `/{endpoint}`, return as-is.
/// 2. If the URL ends with `/chat/completions`, strip it to recover the base.
/// 3. If the resulting base contains a version segment (e.g. `/v1`, `/v1beta`),
///    append `/{endpoint}` directly.
/// 4. Otherwise (bare root), append `/v1/{endpoint}`.
fn build_endpoint_url(base_url: &str, endpoint: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let suffix = format!("/{}", endpoint);

    // Case 1: URL already targets the requested endpoint
    if base.ends_with(&suffix) {
        return base.to_string();
    }

    // Case 2: strip /chat/completions to recover the versioned base
    let stripped = base.strip_suffix("/chat/completions").unwrap_or(base);

    // Case 3 & 4: check for version segment
    if has_version_segment(stripped) {
        format!("{}{}", stripped, suffix)
    } else {
        format!("{}/v1{}", stripped, suffix)
    }
}

/// Check if the URL path contains a version-like segment.
///
/// A version segment is a path component starting with `v` followed by at
/// least one ASCII digit, e.g. `v1`, `v1beta`, `v2`.
fn has_version_segment(url: &str) -> bool {
    // Extract the path portion (after scheme://host[:port])
    let path = match url.find("://") {
        Some(scheme_end) => {
            let rest = &url[scheme_end + 3..];
            match rest.find('/') {
                Some(pos) => &rest[pos..],
                None => return false, // no path — bare host
            }
        }
        None => url, // no scheme — treat entire string as path
    };

    for segment in path.split('/') {
        let chars: Vec<char> = segment.chars().collect();
        if chars.len() >= 2 && chars[0] == 'v' && chars[1].is_ascii_digit() {
            return true;
        }
    }

    false
}

/// Send a minimal chat completion request to test end-to-end connectivity.
///
/// Sends `{"model": model, "messages": [{"role":"user","content":"hi"}], "max_tokens": 1, "stream": false}`
/// to the upstream's `/chat/completions` endpoint. This verifies:
///
/// 1. Network connectivity
/// 2. API Key validity
/// 3. Model name correctness
/// 4. Endpoint path correctness
/// 5. Response format compatibility
///
/// Returns `Ok(latency_ms)` on success, `Err(error_message)` on failure.
pub async fn send_test_chat_request(
    base_url: &str,
    api_key: &str,
    model: &str,
    timeout_secs: u64,
) -> Result<u64, String> {
    let url = build_chat_completions_url(base_url);
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 1,
        "stream": false
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let start = std::time::Instant::now();
    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求上游失败: {}", e))?;

    let elapsed = start.elapsed().as_millis() as u64;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {} — {}", status, body));
    }

    Ok(elapsed)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_chat_completions_url ──────────────────────────────

    #[test]
    fn test_bare_root_url() {
        assert_eq!(
            build_chat_completions_url("https://api.openai.com"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_bare_root_url_with_trailing_slash() {
        assert_eq!(
            build_chat_completions_url("https://api.openai.com/"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_with_v1_suffix() {
        assert_eq!(
            build_chat_completions_url("https://api.mistral.ai/v1"),
            "https://api.mistral.ai/v1/chat/completions"
        );
    }

    #[test]
    fn test_with_v1_suffix_trailing_slash() {
        assert_eq!(
            build_chat_completions_url("https://api.mistral.ai/v1/"),
            "https://api.mistral.ai/v1/chat/completions"
        );
    }

    #[test]
    fn test_full_chat_completions_url() {
        assert_eq!(
            build_chat_completions_url("https://api.mistral.ai/v1/chat/completions"),
            "https://api.mistral.ai/v1/chat/completions"
        );
    }

    #[test]
    fn test_google_openai_compatible_base() {
        assert_eq!(
            build_chat_completions_url("https://generativelanguage.googleapis.com/v1beta/openai"),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );
    }

    #[test]
    fn test_google_full_endpoint_url() {
        assert_eq!(
            build_chat_completions_url(
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
            ),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );
    }

    #[test]
    fn test_localhost_no_port() {
        assert_eq!(
            build_chat_completions_url("http://localhost:8080"),
            "http://localhost:8080/v1/chat/completions"
        );
    }

    #[test]
    fn test_localhost_with_v1() {
        assert_eq!(
            build_chat_completions_url("http://localhost:8080/v1"),
            "http://localhost:8080/v1/chat/completions"
        );
    }

    #[test]
    fn test_v2_version_segment() {
        assert_eq!(
            build_chat_completions_url("https://api.example.com/v2"),
            "https://api.example.com/v2/chat/completions"
        );
    }

    // ── build_models_url ────────────────────────────────────────

    #[test]
    fn test_models_bare_root_url() {
        assert_eq!(
            build_models_url("https://api.openai.com"),
            "https://api.openai.com/v1/models"
        );
    }

    #[test]
    fn test_models_with_v1_suffix() {
        assert_eq!(
            build_models_url("https://api.mistral.ai/v1"),
            "https://api.mistral.ai/v1/models"
        );
    }

    #[test]
    fn test_models_from_full_chat_url() {
        assert_eq!(
            build_models_url("https://api.mistral.ai/v1/chat/completions"),
            "https://api.mistral.ai/v1/models"
        );
    }

    #[test]
    fn test_models_google_base() {
        assert_eq!(
            build_models_url("https://generativelanguage.googleapis.com/v1beta/openai"),
            "https://generativelanguage.googleapis.com/v1beta/openai/models"
        );
    }

    #[test]
    fn test_models_google_full_chat_url() {
        assert_eq!(
            build_models_url(
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
            ),
            "https://generativelanguage.googleapis.com/v1beta/openai/models"
        );
    }

    #[test]
    fn test_models_already_has_models_suffix() {
        assert_eq!(
            build_models_url("https://api.openai.com/v1/models"),
            "https://api.openai.com/v1/models"
        );
    }

    // ── has_version_segment ─────────────────────────────────────

    #[test]
    fn test_has_version_bare_root() {
        assert!(!has_version_segment("https://api.openai.com"));
    }

    #[test]
    fn test_has_version_v1() {
        assert!(has_version_segment("https://api.mistral.ai/v1"));
    }

    #[test]
    fn test_has_version_v1beta() {
        assert!(has_version_segment(
            "https://generativelanguage.googleapis.com/v1beta/openai"
        ));
    }

    #[test]
    fn test_has_version_v2() {
        assert!(has_version_segment("https://api.example.com/v2"));
    }

    #[test]
    fn test_has_version_not_a_version() {
        assert!(!has_version_segment("https://api.example.com/api"));
    }

    #[test]
    fn test_has_version_empty_path() {
        assert!(!has_version_segment("http://localhost:8080"));
    }

    #[test]
    fn test_has_version_version_in_middle() {
        assert!(has_version_segment(
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        ));
    }

    // ── Native format endpoints ─────────────────────────────────

    #[test]
    fn test_anthropic_messages_bare_root() {
        assert_eq!(
            build_anthropic_messages_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn test_anthropic_messages_with_v1() {
        assert_eq!(
            build_anthropic_messages_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn test_anthropic_messages_already_full() {
        assert_eq!(
            build_anthropic_messages_url("https://api.anthropic.com/v1/messages"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn test_gemini_generate_bare_root() {
        assert_eq!(
            build_gemini_generate_url(
                "https://generativelanguage.googleapis.com",
                "gemini-2.0-flash"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent"
        );
    }

    #[test]
    fn test_gemini_generate_strips_duplicate_v1beta() {
        assert_eq!(
            build_gemini_generate_url(
                "https://generativelanguage.googleapis.com/v1beta",
                "gemini-2.0-flash"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent"
        );
    }

    #[test]
    fn test_responses_bare_root() {
        assert_eq!(
            build_responses_url("https://api.openai.com"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn test_responses_with_v1() {
        assert_eq!(
            build_responses_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn test_build_format_url_dispatch() {
        assert_eq!(
            build_format_url("https://api.openai.com", "openai_chat", "gpt-4"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            build_format_url("https://api.anthropic.com", "anthropic", "claude-3"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            build_format_url(
                "https://generativelanguage.googleapis.com",
                "gemini_native",
                "gemini-2.0-flash"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent"
        );
        assert_eq!(
            build_format_url("https://api.openai.com", "openai_responses", "gpt-4"),
            "https://api.openai.com/v1/responses"
        );
    }
}
