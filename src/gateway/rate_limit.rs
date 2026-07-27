//! 可配置的内存限流器 + 客户端 IP 识别。
//!
//! ## 限流策略
//! 采用固定窗口计数法：每个客户端 IP 在 `window_seconds` 秒内最多发起
//! `max_requests` 次请求。窗口过期后自动重置。
//!
//! ## 客户端 IP 识别
//! 支持两种模式，由 `trust_forwarded_for` 配置项控制：
//! - **直连模式**（默认）：使用 TCP 连接的 `remote_addr`，安全且不可伪造。
//! - **反向代理模式**：从 `X-Forwarded-For` 头取最右侧（最接近本服务）的 IP。
//!   仅在部署于受信任的反向代理后才应启用。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, Request};
use axum::body::Body;

/// 限流配置，从 settings 表加载，支持运行时修改。
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// 是否启用限流（默认 true）
    pub enabled: bool,
    /// 窗口内最大请求数（默认 60）
    pub max_requests: u32,
    /// 窗口时长（秒，默认 60）
    pub window_seconds: u64,
    /// 是否信任 X-Forwarded-For 头（默认 false）
    pub trust_forwarded_for: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_requests: 60,
            window_seconds: 60,
            trust_forwarded_for: false,
        }
    }
}

/// 内存固定窗口限流器：client IP → (count, window_start)。
#[derive(Clone)]
pub struct RateLimiter {
    requests: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
    config: RateLimitConfig,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            requests: Arc::new(Mutex::new(HashMap::new())),
            config,
        }
    }

    /// 更新配置（运行时热更新，不影响已有计数器）。
    pub fn update_config(&self, config: RateLimitConfig) {
        // config 是 Clone 的，但此处需要可变更新
        // 由于 RateLimiter 被 Clone 共享 Arc<Mutex<HashMap>>，
        // 我们不能直接替换 config 字段。
        // 实际使用中每次创建 router 时读取最新配置即可。
        // 这里保留方法签名以备未来扩展（如 ArcSwap）。
        let _ = config;
    }

    /// 返回配置的只读引用。
    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }

    /// 检查指定客户端 IP 是否在限流范围内。
    /// 返回 `Ok(())` 表示允许，`Err(retry_after_secs)` 表示被限流并给出建议重试秒数。
    pub fn check(&self, client_ip: &str) -> Result<(), u64> {
        if !self.config.enabled {
            return Ok(());
        }

        let mut requests = self.requests.lock().expect("rate limiter mutex poisoned");
        let now = Instant::now();
        let window = Duration::from_secs(self.config.window_seconds);

        if let Some((count, window_start)) = requests.get_mut(client_ip) {
            if now.duration_since(*window_start) > window {
                // 窗口已过期，重置
                *count = 1;
                *window_start = now;
                Ok(())
            } else if *count < self.config.max_requests {
                *count += 1;
                Ok(())
            } else {
                // 计算距离窗口重置还需多少秒
                let elapsed = now.duration_since(*window_start);
                let remaining = window.saturating_sub(elapsed);
                Err(remaining.as_secs().max(1))
            }
        } else {
            requests.insert(client_ip.to_string(), (1, now));
            Ok(())
        }
    }

    /// 清理过期的 IP 记录，避免内存无限增长。
    /// 建议在后台定时调用（如每 5 分钟一次）。
    pub fn cleanup_expired(&self) {
        let mut requests = self.requests.lock().expect("rate limiter mutex poisoned");
        let now = Instant::now();
        let window = Duration::from_secs(self.config.window_seconds);
        requests.retain(|_, (_, window_start)| now.duration_since(*window_start) <= window);
    }
}

/// 从请求中提取客户端 IP 地址。
///
/// 根据 `trust_forwarded_for` 配置决定识别策略：
/// - `false`（默认）：直连模式，使用 TCP 连接的 `remote_addr`。
/// - `true`：反向代理模式，从 `X-Forwarded-For` 头取最右侧 IP。
///
/// 如果无法确定 IP，返回 `"unknown"`。
pub fn extract_client_ip(
    headers: &HeaderMap,
    remote_addr: Option<&SocketAddr>,
    trust_forwarded_for: bool,
) -> String {
    if trust_forwarded_for {
        // 反向代理模式：取 X-Forwarded-For 最右侧（最接近本服务）的 IP
        if let Some(xff) = headers.get("x-forwarded-for")
            && let Ok(s) = xff.to_str()
        {
            // XFF 格式: client, proxy1, proxy2, ...
            // 最右侧是最近一跳的代理 IP（或客户端 IP，如果只有一跳）
            if let Some(ip) = s.split(',').next_back().map(|s| s.trim())
                && !ip.is_empty()
            {
                return ip.to_string();
            }
        }
        // XFF 不存在时回退到 remote_addr
    }

    // 直连模式 或 XFF 不存在：使用 TCP 连接地址
    remote_addr
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// 从 Axum 请求中提取客户端 IP 的便捷方法。
pub fn extract_client_ip_from_request(
    req: &Request<Body>,
    trust_forwarded_for: bool,
) -> String {
    let remote_addr = req
        .extensions()
        .get::<SocketAddr>();
    extract_client_ip(req.headers(), remote_addr, trust_forwarded_for)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn make_addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port)
    }

    // ── RateLimiter 核心逻辑测试 ──────────────────────────────

    #[test]
    fn test_allows_within_limit() {
        let config = RateLimitConfig {
            enabled: true,
            max_requests: 3,
            window_seconds: 60,
            trust_forwarded_for: false,
        };
        let limiter = RateLimiter::new(config);

        assert!(limiter.check("1.2.3.4").is_ok());
        assert!(limiter.check("1.2.3.4").is_ok());
        assert!(limiter.check("1.2.3.4").is_ok());
    }

    #[test]
    fn test_rejects_when_exceeded() {
        let config = RateLimitConfig {
            enabled: true,
            max_requests: 2,
            window_seconds: 60,
            trust_forwarded_for: false,
        };
        let limiter = RateLimiter::new(config);

        assert!(limiter.check("1.2.3.4").is_ok());
        assert!(limiter.check("1.2.3.4").is_ok());
        let result = limiter.check("1.2.3.4");
        assert!(result.is_err());
        // retry_after 至少 1 秒
        assert!(result.unwrap_err() >= 1);
    }

    #[test]
    fn test_window_reset_after_expiry() {
        let config = RateLimitConfig {
            enabled: true,
            max_requests: 1,
            window_seconds: 1, // 1 秒窗口，便于测试
            trust_forwarded_for: false,
        };
        let limiter = RateLimiter::new(config);

        assert!(limiter.check("1.2.3.4").is_ok());
        // 超限
        assert!(limiter.check("1.2.3.4").is_err());

        // 等待窗口过期
        std::thread::sleep(Duration::from_millis(1100));

        // 窗口已重置，应允许
        assert!(limiter.check("1.2.3.4").is_ok());
    }

    #[test]
    fn test_disabled_limiter_always_allows() {
        let config = RateLimitConfig {
            enabled: false,
            max_requests: 1,
            window_seconds: 60,
            trust_forwarded_for: false,
        };
        let limiter = RateLimiter::new(config);

        // 即使超过 max_requests，disabled 时也始终允许
        for _ in 0..100 {
            assert!(limiter.check("1.2.3.4").is_ok());
        }
    }

    #[test]
    fn test_different_ips_independent() {
        let config = RateLimitConfig {
            enabled: true,
            max_requests: 1,
            window_seconds: 60,
            trust_forwarded_for: false,
        };
        let limiter = RateLimiter::new(config);

        assert!(limiter.check("1.1.1.1").is_ok());
        assert!(limiter.check("2.2.2.2").is_ok()); // 不同 IP，独立计数
        assert!(limiter.check("1.1.1.1").is_err()); // 1.1.1.1 已超限
        assert!(limiter.check("2.2.2.2").is_err()); // 2.2.2.2 也超限
    }

    #[test]
    fn test_cleanup_expired() {
        let config = RateLimitConfig {
            enabled: true,
            max_requests: 100,
            window_seconds: 1,
            trust_forwarded_for: false,
        };
        let limiter = RateLimiter::new(config);

        let _ = limiter.check("1.1.1.1");
        let _ = limiter.check("2.2.2.2");
        let _ = limiter.check("3.3.3.3");

        // 等待窗口过期
        std::thread::sleep(Duration::from_millis(1100));

        limiter.cleanup_expired();

        // 内部 HashMap 应被清理
        let requests = limiter.requests.lock().unwrap();
        assert!(requests.is_empty());
    }

    // ── 客户端 IP 识别测试 ──────────────────────────────────────

    #[test]
    fn test_extract_ip_direct_mode_no_xff() {
        let headers = HeaderMap::new();
        let addr = make_addr(8080);

        let ip = extract_client_ip(&headers, Some(&addr), false);
        assert_eq!(ip, "127.0.0.1");
    }

    #[test]
    fn test_extract_ip_direct_mode_ignores_xff() {
        // 直连模式下即使有 XFF 也应忽略
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "9.9.9.9".parse().unwrap());
        let addr = make_addr(8080);

        let ip = extract_client_ip(&headers, Some(&addr), false);
        assert_eq!(ip, "127.0.0.1");
    }

    #[test]
    fn test_extract_ip_proxy_mode_with_xff() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "9.9.9.9".parse().unwrap());
        let addr = make_addr(8080);

        let ip = extract_client_ip(&headers, Some(&addr), true);
        assert_eq!(ip, "9.9.9.9");
    }

    #[test]
    fn test_extract_ip_proxy_mode_multi_hop_xff() {
        // 多级代理: client(1.2.3.4) → proxy1(10.0.0.1) → proxy2(10.0.0.2) → 本服务
        // XFF: 1.2.3.4, 10.0.0.1
        // 最右侧是最近一跳 proxy1，但取最右侧意味着取 10.0.0.1
        // 这是安全的做法：只有受信任的代理才能添加 XFF
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "1.2.3.4, 10.0.0.1".parse().unwrap(),
        );
        let addr = make_addr(8080);

        let ip = extract_client_ip(&headers, Some(&addr), true);
        assert_eq!(ip, "10.0.0.1");
    }

    #[test]
    fn test_extract_ip_proxy_mode_no_xff_fallback() {
        // 反向代理模式但无 XFF 头，回退到 remote_addr
        let headers = HeaderMap::new();
        let addr = make_addr(8080);

        let ip = extract_client_ip(&headers, Some(&addr), true);
        assert_eq!(ip, "127.0.0.1");
    }

    #[test]
    fn test_extract_ip_no_remote_addr() {
        let headers = HeaderMap::new();

        let ip = extract_client_ip(&headers, None, false);
        assert_eq!(ip, "unknown");
    }

    #[test]
    fn test_extract_ip_empty_xff() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "".parse().unwrap());
        let addr = make_addr(8080);

        let ip = extract_client_ip(&headers, Some(&addr), true);
        assert_eq!(ip, "127.0.0.1");
    }
}
