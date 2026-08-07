pub mod thinking;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thinking_module_exports() {
        // Smoke test: ensure the thinking module is accessible
        let param = thinking::get_thinking_params(
            "DeepSeek",
            &thinking::ThinkingLevel::High,
            "",
        );
        assert!(param.is_some());
        assert_eq!(param.unwrap()["reasoning_effort"], "high");
    }
}
