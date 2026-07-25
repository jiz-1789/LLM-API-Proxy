use serde_json::Value;

/// Replace model name in upstream response to match the pool display name.
pub fn replace_model_name(body: &mut Value, display_name: &str) {
    if let Some(obj) = body.as_object_mut() {
        if let Some(model_value) = obj.get("model") {
            if let Some(model_str) = model_value.as_str() {
                obj.insert("model".to_string(), Value::String(display_name.to_string()));
            }
        }
    }

    // Also handle choices[].message.model (some providers include it in choices)
    if let Some(choices) = body.get_mut("choices").and_then(|c| c.as_array_mut()) {
        for choice in choices.iter_mut() {
            if let Some(msg) = choice.get_mut("message") {
                if let Some(model_value) = msg.get("model") {
                    if let Some(model_str) = model_value.as_str() {
                        msg.as_object_mut().unwrap().insert(
                            "model".to_string(),
                            Value::String(display_name.to_string()),
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_model_in_top_level() {
        let mut body = serde_json::json!({
            "id": "chatcmpl-123",
            "model": "deepseek-v4",
            "choices": []
        });
        replace_model_name(&mut body, "grok-4.5");
        assert_eq!(body["model"], "grok-4.5");
    }

    #[test]
    fn test_replace_model_in_choices() {
        let mut body = serde_json::json!({
            "model": "deepseek-v4",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "model": "deepseek-v4",
                        "content": "Hello"
                    }
                }
            ]
        });
        replace_model_name(&mut body, "grok-4.5");
        assert_eq!(body["model"], "grok-4.5");
        assert_eq!(body["choices"][0]["message"]["model"], "grok-4.5");
    }

    #[test]
    fn test_no_model_field_unchanged() {
        let mut body = serde_json::json!({
            "data": [{ "embedding": [0.1, 0.2] }]
        });
        replace_model_name(&mut body, "grok-4.5");
        assert_eq!(body["data"][0]["embedding"][0], 0.1);
    }
}
