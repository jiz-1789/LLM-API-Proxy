//! P0-12: Streaming proxy integration tests.
//!
//! These tests verify SSE streaming behavior including model replacement,
//! error detection in stream chunks, token usage extraction, and
//! `data: [DONE]` handling.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::tests::common::TestEnv;

/// Read the full SSE response body as a string.
async fn read_sse_body(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

/// Helper: build an SSE response from wiremock.
fn sse_response(chunks: &[&str]) -> ResponseTemplate {
    let body = chunks.join("\n\n");
    ResponseTemplate::new(200)
        .insert_header("Content-Type", "text/event-stream")
        .set_body_string(body)
}

#[tokio::test]
async fn test_sse_stream_passthrough_and_model_replacement() {
    let mock_server = MockServer::start().await;

    let sse_data = vec![
        r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","model":"test-model","choices":[{"delta":{"role":"assistant"},"index":0}]}"#,
        r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","model":"test-model","choices":[{"delta":{"content":"Hello"},"index":0}]}"#,
        r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","model":"test-model","choices":[{"delta":{"content":" world"},"index":0}]}"#,
        "data: [DONE]",
    ];

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&sse_data))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("StreamProvider", &mock_server.uri());
    env.create_pool("stream-model", "Stream Pool", &[upstream_id]);

    let resp = env.send_chat_request("stream-model", true).await;

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/event-stream"
    );

    let body = read_sse_body(resp).await;

    // Model should be replaced with pool's display name in all chunks
    assert!(body.contains(r#""model":"Stream Pool""#), "Model should be replaced with display name");
    assert!(!body.contains(r#""model":"test-model""#), "Original model name should not appear");

    // Should contain [DONE]
    assert!(body.contains("data: [DONE]"));
}

#[tokio::test]
async fn test_anthropic_stream_converts_chat_sse_to_anthropic_events() {
    let mock_server = MockServer::start().await;

    let sse_data = vec![
        r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","model":"test-model","choices":[{"delta":{"role":"assistant"},"index":0}]}"#,
        r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","model":"test-model","choices":[{"delta":{"content":"Hello"},"index":0}]}"#,
        r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","model":"test-model","choices":[{"delta":{"content":" world"},"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#,
        "data: [DONE]",
    ];

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&sse_data))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("StreamProvider", &mock_server.uri());
    env.create_pool("stream-model", "Stream Pool", &[upstream_id]);

    let resp = env.send_anthropic_stream_request("stream-model").await;

    assert_eq!(resp.status(), 200);
    let body = read_sse_body(resp).await;

    // Anthropic SSE lifecycle (matches the reference implementation):
    // message_start -> content_block_start(text) -> text_delta -> message_delta -> message_stop
    assert!(body.starts_with("event: message_start"), "got: {body}");
    assert!(body.contains(r#""type":"message""#), "got: {body}");
    assert!(body.contains("content_block_delta"), "got: {body}");
    assert!(body.contains(r#""type":"text_delta""#), "got: {body}");
    assert!(body.contains(r#""text":"Hello""#), "got: {body}");
    // Completion: message_delta + message_stop
    assert!(body.contains("message_delta"), "got: {body}");
    assert!(body.contains("event: message_stop"), "got: {body}");
    assert!(body.contains(r#""stop_reason":"end_turn""#), "got: {body}");
    // No [DONE] after the Anthropic stream — it ends at message_stop.
    assert!(!body.contains("data: [DONE]"), "got: {body}");
}

#[tokio::test]
async fn test_sse_stream_error_detection() {
    let mock_server = MockServer::start().await;

    // Stream that contains an error in a chunk
    let sse_data = vec![
        r#"data: {"id":"chatcmpl-1","model":"test-model","choices":[]}"#,
        r#"data: {"error":{"message":"rate limit exceeded","type":"rate_limit_error"}}"#,
    ];

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&sse_data))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("ErrStreamProvider", &mock_server.uri());
    env.create_pool("err-stream-model", "Err Stream Pool", &[upstream_id]);

    let resp = env.send_chat_request("err-stream-model", true).await;
    assert_eq!(resp.status(), 200);

    let body = read_sse_body(resp).await;
    // The error chunk should be forwarded to the client
    assert!(body.contains("rate limit exceeded"));

    // Verify the log status was updated to 500 (error detected mid-stream)
    let logs = env.db.get_recent_logs(
        &crate::db::LogFilter {
            limit: 5,
            offset: 0,
            ..Default::default()
        },
    ).unwrap();
    assert!(!logs.is_empty());
    // The stream spawned a task that updates the log status asynchronously.
    // Give it a moment to complete.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let logs = env.db.get_recent_logs(
        &crate::db::LogFilter {
            limit: 5,
            offset: 0,
            ..Default::default()
        },
    ).unwrap();
    let error_log = logs.iter().find(|l| l.status_code == 500);
    assert!(error_log.is_some(), "Stream error should update log status to 500");
}

#[tokio::test]
async fn test_sse_token_usage_extraction() {
    let mock_server = MockServer::start().await;

    // Final chunk contains usage info (when stream_options.include_usage is set)
    let sse_data = vec![
        r#"data: {"id":"chatcmpl-1","model":"test-model","choices":[{"delta":{"content":"Hi"},"index":0}]}"#,
        r#"data: {"id":"chatcmpl-1","model":"test-model","choices":[],"usage":{"prompt_tokens":8,"completion_tokens":2,"total_tokens":10}}"#,
        "data: [DONE]",
    ];

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&sse_data))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("UsageProvider", &mock_server.uri());
    env.create_pool("usage-model", "Usage Pool", &[upstream_id]);

    let resp = env.send_chat_request("usage-model", true).await;
    assert_eq!(resp.status(), 200);

    // Consume the body so the spawned stream task can complete
    let _body = read_sse_body(resp).await;

    // Wait for the async log update to complete
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let logs = env.db.get_recent_logs(
        &crate::db::LogFilter {
            limit: 5,
            offset: 0,
            ..Default::default()
        },
    ).unwrap();
    assert!(!logs.is_empty());
    let log = &logs[0];
    assert_eq!(log.prompt_tokens, 8, "Prompt tokens should be extracted from stream");
    assert_eq!(log.completion_tokens, 2, "Completion tokens should be extracted from stream");
    assert_eq!(log.total_tokens, 10, "Total tokens should be extracted from stream");
}

#[tokio::test]
async fn test_sse_done_marker() {
    let mock_server = MockServer::start().await;

    let sse_data = vec![
        r#"data: {"id":"chatcmpl-1","model":"test-model","choices":[{"delta":{"content":"Bye"},"index":0}]}"#,
        "data: [DONE]",
    ];

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&sse_data))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("DoneProvider", &mock_server.uri());
    env.create_pool("done-model", "Done Pool", &[upstream_id]);

    let resp = env.send_chat_request("done-model", true).await;
    assert_eq!(resp.status(), 200);

    let body = read_sse_body(resp).await;
    // [DONE] should be present as the last data line
    assert!(body.contains("data: [DONE]"));
    // Should also contain the content chunk with replaced model
    assert!(body.contains(r#""model":"Done Pool""#));
}

#[tokio::test]
async fn test_sse_stream_has_x_request_id() {
    let mock_server = MockServer::start().await;

    let sse_data = vec![
        r#"data: {"id":"chatcmpl-1","model":"test-model","choices":[{"delta":{"content":"Hi"},"index":0}]}"#,
        "data: [DONE]",
    ];

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&sse_data))
        .mount(&mock_server)
        .await;

    let env = TestEnv::new().await;
    let upstream_id = env.create_upstream("TraceStreamProvider", &mock_server.uri());
    env.create_pool("trace-stream-model", "Trace Stream Pool", &[upstream_id]);

    let resp = env.send_chat_request("trace-stream-model", true).await;

    assert_eq!(resp.status(), 200);
    let trace_id = resp.headers().get("x-request-id");
    assert!(trace_id.is_some(), "Stream response should have X-Request-Id header");

    // Consume body
    let _body = read_sse_body(resp).await;
}

#[tokio::test]
async fn test_sse_failover_to_second_upstream() {
    let bad_server = MockServer::start().await;
    let good_server = MockServer::start().await;

    // First upstream returns 500
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("error"))
        .mount(&bad_server)
        .await;

    // Second upstream returns a successful stream
    let sse_data = vec![
        r#"data: {"id":"chatcmpl-1","model":"test-model","choices":[{"delta":{"content":"Fallback stream"},"index":0}]}"#,
        "data: [DONE]",
    ];

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&sse_data))
        .mount(&good_server)
        .await;

    let env = TestEnv::new().await;
    let bad_id = env.create_upstream("BadStream", &bad_server.uri());
    let good_id = env.create_upstream("GoodStream", &good_server.uri());
    env.create_pool("failover-stream-model", "Failover Stream Pool", &[bad_id, good_id]);

    let resp = env.send_chat_request("failover-stream-model", true).await;
    assert_eq!(resp.status(), 200);

    let body = read_sse_body(resp).await;
    assert!(body.contains("Fallback stream"), "Should receive content from second upstream");
    assert!(body.contains(r#""model":"Failover Stream Pool""#));
}
