//! P0-11: Gateway core path integration tests.
//!
//! These tests use `wiremock` to simulate upstream LLM providers and
//! verify the gateway's routing, authentication, failover, and logging
//! behavior through the full request lifecycle.

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::tests::common::TestEnv;

/// Helper: read response body as JSON.
async fn read_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// Helper: read response body as text.
async fn read_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

// ── Authentication tests ──────────────────────────────────────

#[tokio::test]
async fn test_valid_api_key_forwards_request() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer sk-upstream-test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "model": "test-model",
            "choices": [{"message": {"role": "assistant", "content": "Hello!"}, "index": 0}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("TestProvider", &mock_server.uri());
    env.create_pool("test-model", "Test Pool", &[upstream_id]);

    let resp = env.send_chat_request("test-model", false).await;

    assert_eq!(resp.status(), 200);
    let body = read_json(resp).await;
    // Model should be replaced with the pool's display name
    assert_eq!(body["model"], "Test Pool");
    assert_eq!(body["choices"][0]["message"]["content"], "Hello!");
}

#[tokio::test]
async fn test_invalid_api_key_returns_401() {
    let env = TestEnv::new().await;

    let resp = env.send_chat_request_no_auth("test-model").await;

    assert_eq!(resp.status(), 401);
    let body = read_json(resp).await;
    assert!(body["error"]["message"].as_str().unwrap().contains("authorization"));
}

// ── Request validation tests ─────────────────────────────────

#[tokio::test]
async fn test_missing_model_returns_400() {
    let env = TestEnv::new().await;

    let resp = env.send_chat_request_no_model().await;

    assert_eq!(resp.status(), 400);
    let body = read_json(resp).await;
    assert_eq!(body["error"]["code"], "missing_model");
}

#[tokio::test]
async fn test_unknown_model_returns_404() {
    let env = TestEnv::new().await;

    let resp = env.send_chat_request("nonexistent-model", false).await;

    assert_eq!(resp.status(), 404);
    let body = read_json(resp).await;
    assert_eq!(body["error"]["code"], "model_not_found");
}

// ── Failover tests ───────────────────────────────────────────

#[tokio::test]
async fn test_failover_on_5xx() {
    let bad_server = MockServer::start().await;
    let good_server = MockServer::start().await;

    // First upstream returns 500
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
        .mount(&bad_server)
        .await;

    // Second upstream returns 200
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-456",
            "object": "chat.completion",
            "model": "test-model",
            "choices": [{"message": {"role": "assistant", "content": "Fallback!"}, "index": 0}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        })))
        .mount(&good_server)
        .await;

    let env = TestEnv::new().await;
    let bad_id = env.create_upstream("BadProvider", &bad_server.uri());
    let good_id = env.create_upstream("GoodProvider", &good_server.uri());
    env.create_pool("failover-model", "Failover Pool", &[bad_id, good_id]);

    let resp = env.send_chat_request("failover-model", false).await;

    assert_eq!(resp.status(), 200);
    let body = read_json(resp).await;
    assert_eq!(body["choices"][0]["message"]["content"], "Fallback!");
}

#[tokio::test]
async fn test_all_upstreams_fail_returns_502() {
    let bad_server1 = MockServer::start().await;
    let bad_server2 = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("error 1"))
        .mount(&bad_server1)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("error 2"))
        .mount(&bad_server2)
        .await;

    let env = TestEnv::new().await;
    let id1 = env.create_upstream("Bad1", &bad_server1.uri());
    let id2 = env.create_upstream("Bad2", &bad_server2.uri());
    env.create_pool("all-fail-model", "All Fail Pool", &[id1, id2]);

    let resp = env.send_chat_request("all-fail-model", false).await;

    assert_eq!(resp.status(), 502);
    let body = read_json(resp).await;
    assert_eq!(body["error"]["code"], "all_upstreams_failed");
    assert!(body["error"]["details"].is_array());
    assert_eq!(body["error"]["details"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_all_upstreams_disabled_returns_503() {
    let mock_server = MockServer::start().await;

    let env = TestEnv::new().await;
    let id1 = env.create_disabled_upstream("Disabled1", &mock_server.uri());
    let id2 = env.create_disabled_upstream("Disabled2", &mock_server.uri());
    env.create_pool("disabled-model", "Disabled Pool", &[id1, id2]);

    let resp = env.send_chat_request("disabled-model", false).await;

    assert_eq!(resp.status(), 503);
    let body = read_json(resp).await;
    assert_eq!(body["error"]["code"], "all_upstreams_disabled");
}

#[tokio::test]
async fn test_4xx_does_not_trigger_failover() {
    let bad_server = MockServer::start().await;
    let good_server = MockServer::start().await;

    // First upstream returns 400 (client error - should NOT failover)
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
        .mount(&bad_server)
        .await;

    // Second upstream should NOT be called
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "test-model",
            "choices": [{"message": {"content": "should not reach here"}}]
        })))
        .mount(&good_server)
        .await;

    let env = TestEnv::new().await;
    let bad_id = env.create_upstream("Bad", &bad_server.uri());
    let good_id = env.create_upstream("Good", &good_server.uri());
    env.create_pool("no-failover-model", "No Failover Pool", &[bad_id, good_id]);

    let resp = env.send_chat_request("no-failover-model", false).await;

    // Should get 502 because failover was NOT triggered (4xx = no failover)
    // and the first upstream failed
    assert_eq!(resp.status(), 502);
}

// ── Request logging test ─────────────────────────────────────

#[tokio::test]
async fn test_request_log_recorded() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-789",
            "object": "chat.completion",
            "model": "test-model",
            "choices": [{"message": {"role": "assistant", "content": "Logged!"}, "index": 0}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("LogProvider", &mock_server.uri());
    env.create_pool("log-model", "Log Pool", &[upstream_id]);

    let resp = env.send_chat_request("log-model", false).await;
    assert_eq!(resp.status(), 200);

    // Verify the request was logged in the database
    let logs = env.db.get_recent_logs(
        &crate::db::LogFilter {
            limit: 10,
            offset: 0,
            ..Default::default()
        },
    ).unwrap();

    assert!(!logs.is_empty(), "Request log should be recorded");
    let log = &logs[0];
    assert_eq!(log.status_code, 200);
    assert_eq!(log.pool_name, Some("log-model".to_string()));
    assert_eq!(log.prompt_tokens, 10);
    assert_eq!(log.completion_tokens, 5);
    assert_eq!(log.total_tokens, 15);
}

// ── X-Request-Id header test ─────────────────────────────────

#[tokio::test]
async fn test_response_has_x_request_id() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "test-model",
            "choices": [{"message": {"content": "hi"}}]
        })))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("TraceProvider", &mock_server.uri());
    env.create_pool("trace-model", "Trace Pool", &[upstream_id]);

    let resp = env.send_chat_request("trace-model", false).await;

    assert_eq!(resp.status(), 200);
    let trace_id = resp.headers().get("x-request-id");
    assert!(trace_id.is_some(), "Response should have X-Request-Id header");
    assert!(!trace_id.unwrap().is_empty());
}

// ── Error response format test ───────────────────────────────

#[tokio::test]
async fn test_error_response_format() {
    let env = TestEnv::new().await;
    let resp = env.send_chat_request("nonexistent", false).await;

    assert_eq!(resp.status(), 404);
    let body = read_json(resp).await;
    // Verify OpenAI-compatible error format
    assert!(body["error"]["message"].is_string());
    assert!(body["error"]["type"].is_string());
    assert!(body["error"]["code"].is_string());
    assert!(body["error"]["param"].is_null());
}
