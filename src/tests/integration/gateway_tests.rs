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

// ── OpenAI Responses API endpoint ─────────────────────────────

#[tokio::test]
async fn test_responses_endpoint_normalizes_and_converts_output() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "model": "test-model",
            "choices": [{"message": {"role": "assistant", "content": "Hello from responses!"}, "index": 0}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("TestProvider", &mock_server.uri());
    env.create_pool("test-model", "Test Pool", &[upstream_id]);

    let resp = env.send_responses_request("test-model").await;

    assert_eq!(resp.status(), 200);
    let body = read_json(resp).await;
    // Responses API output shape
    assert_eq!(body["object"], "response");
    assert_eq!(body["status"], "completed");
    assert_eq!(body["output"][0]["type"], "message");
    assert_eq!(body["output"][0]["content"][0]["text"], "Hello from responses!");
    assert_eq!(body["usage"]["input_tokens"], 10);
    assert_eq!(body["usage"]["output_tokens"], 5);
}

// ── Anthropic Messages API endpoint ──────────────────────────

#[tokio::test]
async fn test_anthropic_messages_endpoint_normalizes_and_converts() {
    let mock_server = MockServer::start().await;

    // The gateway forwards the normalized Chat request to the upstream.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer sk-upstream-test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "model": "test-model",
            "choices": [{"message": {"role": "assistant", "content": "Hello from anthropic!"}, "index": 0}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("TestProvider", &mock_server.uri());
    env.create_pool("test-model", "Test Pool", &[upstream_id]);

    let resp = env.send_anthropic_request("test-model").await;

    assert_eq!(resp.status(), 200);
    let body = read_json(resp).await;
    // Anthropic Messages output shape
    assert_eq!(body["type"], "message");
    assert_eq!(body["role"], "assistant");
    assert_eq!(body["model"], "Test Pool");
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(body["content"][0]["type"], "text");
    assert_eq!(body["content"][0]["text"], "Hello from anthropic!");
    assert_eq!(body["usage"]["input_tokens"], 10);
    assert_eq!(body["usage"]["output_tokens"], 5);
}

// ── Gemini Native API endpoint ───────────────────────────────

#[tokio::test]
async fn test_gemini_generate_endpoint_normalizes_and_converts() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "model": "test-model",
            "choices": [{"message": {"role": "assistant", "content": "Hello from gemini!"}, "index": 0}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("TestProvider", &mock_server.uri());
    env.create_pool("test-model", "Test Pool", &[upstream_id]);

    let resp = env.send_gemini_request("test-model").await;

    assert_eq!(resp.status(), 200);
    let body = read_json(resp).await;
    // Gemini Native output shape
    assert_eq!(body["candidates"][0]["content"]["parts"][0]["text"], "Hello from gemini!");
    assert_eq!(body["candidates"][0]["finishReason"], "STOP");
    assert_eq!(body["modelVersion"], "Test Pool");
    assert_eq!(body["usageMetadata"]["promptTokenCount"], 10);
    assert_eq!(body["usageMetadata"]["candidatesTokenCount"], 5);
}

#[tokio::test]
async fn test_gemini_invalid_path_returns_400() {
    let env = TestEnv::new().await;

    use tower::ServiceExt;
    let body = serde_json::json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]});
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/v1beta/models/:generateContent")
        .header("x-goog-api-key", crate::tests::common::TEST_API_KEY)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = env.router.clone().oneshot(request).await.unwrap();
    assert_eq!(resp.status(), 400);
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

// ── Embedded error failover test ─────────────────────────────
// HTTP 200 but body contains "error" field → should trigger failover

#[tokio::test]
async fn test_embedded_error_triggers_failover() {
    let fake_ok_server = MockServer::start().await;
    let good_server = MockServer::start().await;

    // First upstream returns 200 but with error in body ("fake success")
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "error": {"message": "model overloaded", "type": "server_error"}
        })))
        .mount(&fake_ok_server)
        .await;

    // Second upstream returns a real success
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-embed",
            "model": "test-model",
            "choices": [{"message": {"role": "assistant", "content": "Real success"}, "index": 0}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        })))
        .mount(&good_server)
        .await;

    let env = TestEnv::new().await;
    let fake_id = env.create_upstream("FakeOk", &fake_ok_server.uri());
    let good_id = env.create_upstream("RealGood", &good_server.uri());
    env.create_pool("embed-failover-model", "Embed Failover Pool", &[fake_id, good_id]);

    let resp = env.send_chat_request("embed-failover-model", false).await;

    assert_eq!(resp.status(), 200);
    let body = read_json(resp).await;
    assert_eq!(body["choices"][0]["message"]["content"], "Real success");
}

// ── Auth failure failover test ───────────────────────────────
// 401 from first upstream should trigger failover (different upstreams have different keys)

#[tokio::test]
async fn test_auth_failure_triggers_failover() {
    let auth_fail_server = MockServer::start().await;
    let good_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&auth_fail_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-auth-failover",
            "model": "test-model",
            "choices": [{"message": {"role": "assistant", "content": "Auth failover success"}, "index": 0}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        })))
        .mount(&good_server)
        .await;

    let env = TestEnv::new().await;
    let auth_fail_id = env.create_upstream("AuthFail", &auth_fail_server.uri());
    let good_id = env.create_upstream("AuthGood", &good_server.uri());
    env.create_pool("auth-failover-model", "Auth Failover Pool", &[auth_fail_id, good_id]);

    let resp = env.send_chat_request("auth-failover-model", false).await;

    assert_eq!(resp.status(), 200);
    let body = read_json(resp).await;
    assert_eq!(body["choices"][0]["message"]["content"], "Auth failover success");
}

// ── Round-robin distribution test ────────────────────────────

#[tokio::test]
async fn test_round_robin_distributes_requests() {
    let server_a = MockServer::start().await;
    let server_b = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "a",
            "model": "test-model",
            "choices": [{"message": {"content": "from-a"}, "index": 0}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })))
        .mount(&server_a)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "b",
            "model": "test-model",
            "choices": [{"message": {"content": "from-b"}, "index": 0}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })))
        .mount(&server_b)
        .await;

    let env = TestEnv::new().await;
    let id_a = env.create_upstream("ProviderA", &server_a.uri());
    let id_b = env.create_upstream("ProviderB", &server_b.uri());
    env.create_pool("rr-model", "RR Pool", &[id_a, id_b]);

    // Send 4 requests — should alternate between A and B
    let mut responses = Vec::new();
    for _ in 0..4 {
        let resp = env.send_chat_request("rr-model", false).await;
        let body = read_json(resp).await;
        responses.push(body["choices"][0]["message"]["content"].as_str().unwrap().to_string());
    }

    // With round_robin strategy, requests should be distributed across both upstreams
    let count_a = responses.iter().filter(|r| r == &"from-a").count();
    let count_b = responses.iter().filter(|r| r == &"from-b").count();
    assert_eq!(count_a + count_b, 4, "All requests should succeed");
    assert!(count_a > 0 && count_b > 0, "Both upstreams should receive traffic (got A={}, B={})", count_a, count_b);
}

// ── Health status update test ────────────────────────────────

#[tokio::test]
async fn test_health_status_updates_on_success_and_failure() {
    let good_server = MockServer::start().await;
    let bad_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "health-ok",
            "model": "test-model",
            "choices": [{"message": {"content": "ok"}, "index": 0}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })))
        .mount(&good_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("error"))
        .mount(&bad_server)
        .await;

    let env = TestEnv::new().await;
    let good_id = env.create_upstream("HealthGood", &good_server.uri());
    let bad_id = env.create_upstream("HealthBad", &bad_server.uri());
    env.create_pool("health-model", "Health Pool", &[bad_id.clone(), good_id.clone()]);

    // Send a request — bad upstream fails, good upstream succeeds (failover)
    let resp = env.send_chat_request("health-model", false).await;
    assert_eq!(resp.status(), 200);

    // Verify health status was updated
    let statuses = env.db.get_upstream_status_summary().unwrap();
    let bad_status = statuses.iter().find(|s| s.id == bad_id).unwrap();
    let good_status = statuses.iter().find(|s| s.id == good_id).unwrap();

    assert_eq!(bad_status.status, "degraded", "Failed upstream should be degraded");
    assert!(bad_status.failure_count > 0, "Failed upstream should have failure_count > 0");
    assert!(bad_status.last_failure_time.is_some(), "Failed upstream should have last_failure_time");

    assert_eq!(good_status.status, "healthy", "Successful upstream should be healthy");
    assert_eq!(good_status.failure_count, 0, "Successful upstream should have failure_count = 0");
    assert!(good_status.last_success_time.is_some(), "Successful upstream should have last_success_time");
}

// ── Model replacement in non-streaming response test ─────────

#[tokio::test]
async fn test_model_replaced_in_response() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-replace",
            "object": "chat.completion",
            "model": "actual-upstream-model",
            "choices": [{"message": {"role": "assistant", "content": "test"}, "index": 0}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 1, "total_tokens": 4}
        })))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("ReplaceProvider", &mock_server.uri());
    env.create_pool("replace-model", "Display Name Pool", &[upstream_id]);

    let resp = env.send_chat_request("replace-model", false).await;
    assert_eq!(resp.status(), 200);

    let body = read_json(resp).await;
    assert_eq!(body["model"], "Display Name Pool", "Model should be replaced with pool display name");
    assert_ne!(body["model"], "actual-upstream-model", "Original model name should not appear");
}
