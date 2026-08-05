pub mod thinking;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thinking_module_exports() {
        // Smoke test: ensure the thinking module is accessible
        let param = thinking::get_thinking_param("DeepSeek", true);
        assert!(param.is_some());
    }
}
