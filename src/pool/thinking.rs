/// Thinking mode parameter injection by vendor type.

/// Determines which thinking parameter to inject based on vendor name.
pub fn get_thinking_param(vendor: &str, enabled: bool) -> Option<serde_json::Value> {
    if !enabled {
        return None;
    }

    let vendor_lower = vendor.to_lowercase();

    match vendor_lower.as_str() {
        v if v.contains("deepseek") || v.contains("ds") => {
            Some(serde_json::json!({ "reasoning": true }))
        }
        v if v.contains("openai") || v.contains("gpt") => {
            Some(serde_json::json!({ "reasoning_effort": "high" }))
        }
        v if v.contains("claude") || v.contains("anthropic") => {
            Some(serde_json::json!({ "thinking": { "type": "enabled" } }))
        }
        _ => None,
    }
}

/// Merge thinking parameters into the request body.
pub fn merge_thinking_params(body: &mut serde_json::Value, params: &Option<serde_json::Value>) {
    if let Some(thinking_param) = params {
        if let Some(obj) = body.as_object_mut() {
            if let Some(thinking_obj) = thinking_param.as_object() {
                for (key, value) in thinking_obj {
                    obj.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deepseek_thinking_param() {
        let param = get_thinking_param("DeepSeek", true);
        assert!(param.is_some());
        assert_eq!(param.unwrap()["reasoning"], true);
    }

    #[test]
    fn test_openai_thinking_param() {
        let param = get_thinking_param("OpenAI", true);
        assert!(param.is_some());
        assert_eq!(param.unwrap()["reasoning_effort"], "high");
    }

    #[test]
    fn test_disabled_returns_none() {
        let param = get_thinking_param("DeepSeek", false);
        assert!(param.is_none());
    }

    #[test]
    fn test_unknown_vendor_returns_none() {
        let param = get_thinking_param("UnknownVendor", true);
        assert!(param.is_none());
    }

    #[test]
    fn test_merge_params_into_body() {
        let mut body = serde_json::json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let params = Some(serde_json::json!({ "reasoning": true }));
        merge_thinking_params(&mut body, &params);
        assert_eq!(body["reasoning"], true);
        assert_eq!(body["model"], "test-model");
    }
}
