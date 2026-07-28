//! 可配置的内存限流器 + 客户端 IP 识别。
//!
//! ## 限流策略
//! 采用固定窗口计数法：每个客户端 IP 在 `window_seconds` 秒内最多发起
//! `max_requests` 次请求。窗口过期后自动重置。
//!
//! ## 并发安全（P2-16）
//! 使用 `DashMap` 替代 `Mutex<HashMap>`，提供分片级别的并发访问，
//! 避免全局锁争用。每个 key 的读写操作只持有对应分片的锁。
//!
//! ## 状态持久化（P2-15）
//! 限流计数通过 `SystemTime`（而非 `Instant`）记录窗口起始时间，
//! 使得状态可以序列化到 SQLite。启动时从数据库加载，后台定期持久化，
//! 应用重启后限流计数不丢失。
//!
//! ## 客户端 IP 识别
//! 支持两种模式，由 `trust_forwarded_for` 配置项控制：
//! - **直连模式**（默认）：使用 TCP 连接的 `remote_addr`，安全且不可伪造。
//! - **反向代理模式**：从 `X-Forwarded-For` 头取最右侧（最接近本服务）的 IP。
//!   仅在部署于受信任的反向代理后才应启用。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{HeaderMap, Request};
use dashmap::DashMap;
use tracing::{info, warn};

use crate::db::Database;

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
///
/// 使用 `DashMap` 实现并发安全的 per-key 访问，
/// 使用 `SystemTime` 使窗口起始时间可持久化到数据库。
#[derive(Clone)]
pub struct RateLimiter {
    requests: Arc<DashMap<String, (u32, SystemTime)>>,
    config: RateLimitConfig,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            requests: Arc::new(DashMap::new()),
            config,
        }
    }

    /// 更新配置（运行时热更新，不影响已有计数器）。
    pub fn update_config(&self, config: RateLimitConfig) {
        // config 是 Clone 的，但此处需要可变更新
        // 由于 RateLimiter 被 Clone 共享 Arc<DashMap>，
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

        let now = SystemTime::now();
        let window = Duration::from_secs(self.config.window_seconds);

        // DashMap entry() provides per-key locking without global lock
        if let Some(mut entry) = self.requests.get_mut(client_ip) {
            let (count, window_start) = entry.value_mut();
            if now.duration_since(*window_start).map(|d| d > window).unwrap_or(true) {
                // 窗口已过期，重置
                *count = 1;
                *window_start = now;
                Ok(())
            } else if *count < self.config.max_requests {
                *count += 1;
                Ok(())
            } else {
                // 计算距离窗口重置还需多少秒
                let elapsed = now.duration_since(*window_start).unwrap_or(Duration::ZERO);
                let remaining = window.saturating_sub(elapsed);
                Err(remaining.as_secs().max(1))
            }
        } else {
            self.requests.insert(client_ip.to_string(), (1, now));
            Ok(())
        }
    }

    /// 清理过期的 IP 记录，避免内存无限增长。
    /// 建议在后台定时调用（如每 5 分钟一次）。
    pub fn cleanup_expired(&self) {
        let now = SystemTime::now();
        let window = Duration::from_secs(self.config.window_seconds);
        self.requests.retain(|_, (_, window_start)| {
            now.duration_since(*window_start).map(|d| d <= window).unwrap_or(true)
        });
    }

    // ========================================================================
    // Persistence (P2-15)
    // ========================================================================

    /// Load rate limit state from the database into the in-memory map.
    /// Expired entries (window already passed) are discarded.
    pub fn load_from_db(&self, db: &Database) {
        match db.load_rate_limit_state() {
            Ok(entries) => {
                let now = SystemTime::now();
                let window = Duration::from_secs(self.config.window_seconds);
                let mut loaded = 0;
                for (ip, count, window_start_secs) in entries {
                    // Convert unix secs back to SystemTime
                    let window_start = UNIX_EPOCH + Duration::from_secs(window_start_secs as u64);
                    // Skip expired entries
                    if now.duration_since(window_start).map(|d| d <= window).unwrap_or(false) {
                        self.requests.insert(ip, (count, window_start));
                        loaded += 1;
                    }
                }
                if loaded > 0 {
                    info!("Loaded {} rate limit entries from database", loaded);
                }
            }
            Err(e) => {
                warn!("Failed to load rate limit state from database: {}", e);
            }
        }
    }

    /// Persist the current in-memory rate limit state to the database.
    pub fn persist_to_db(&self, db: &Database) {
        let now = SystemTime::now();
        let window = Duration::from_secs(self.config.window_seconds);

        // Collect non-expired entries
        let entries: Vec<(String, u32, i64)> = self
            .requests
            .iter()
            .filter_map(|ref_entry| {
                let (ip, (count, window_start)) = ref_entry.pair();
                // Skip expired entries
                if now.duration_since(*window_start).map(|d| d <= window).unwrap_or(false) {
                    let secs = window_start
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    Some((ip.clone(), *count, secs))
                } else {
                    None
                }
            })
            .collect();

        if entries.is_empty() {
            // Clear the table to remove stale entries
            if let Err(e) = db.clear_rate_limit_state() {
                warn!("Failed to clear rate limit state: {}", e);
            }
            return;
        }

        // Clear and re-insert (simpler than per-key upsert for batch)
        if let Err(e) = db.clear_rate_limit_state() {
            warn!("Failed to clear rate limit state: {}", e);
            return;
        }
        if let Err(e) = db.save_rate_limit_state(&entries) {
            warn!("Failed to persist rate limit state: {}", e);
        } else {
            info!("Persisted {} rate limit entries to database", entries.len());
        }
    }

    /// Start a background task that periodically persists the rate limit state.
    /// Runs every 5 minutes.
    pub fn start_persist_task(self, db: Arc<Database>) {
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    warn!("Failed to create tokio runtime for rate limit persistence: {}", e);
                    return;
                }
            };
            rt.block_on(async move {
                let interval = Duration::from_secs(300); // 5 minutes
                loop {
                    tokio::time::sleep(interval).await;
                    // Cleanup expired entries first
                    self.cleanup_expired();
                    // Persist to database
                    self.persist_to_db(&db);
                }
            });
        });
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

        // 内部 DashMap 应被清理
        assert!(limiter.requests.is_empty());
    }

    // ── 并发安全测试（P2-16） ─────────────────────────────────

    #[test]
    fn test_concurrent_access_different_ips() {
        let config = RateLimitConfig {
            enabled: true,
            max_requests: 1000,
            window_seconds: 60,
            trust_forwarded_for: false,
        };
        let limiter = RateLimiter::new(config);

        let mut handles = vec![];
        for i in 0..10 {
            let lim = limiter.clone();
            handles.push(std::thread::spawn(move || {
                let ip = format!("10.0.0.{}", i);
                for _ in 0..100 {
                    assert!(lim.check(&ip).is_ok());
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // Each IP should have 100 requests
        for i in 0..10 {
            let ip = format!("10.0.0.{}", i);
            let entry = limiter.requests.get(&ip).unwrap();
            assert_eq!(entry.0, 100);
        }
    }

    #[test]
    fn test_concurrent_access_same_ip() {
        let config = RateLimitConfig {
            enabled: true,
            max_requests: 500,
            window_seconds: 60,
            trust_forwarded_for: false,
        };
        let limiter = RateLimiter::new(config);

        let mut handles = vec![];
        for _ in 0..10 {
            let lim = limiter.clone();
            handles.push(std::thread::spawn(move || {
                let mut ok = 0;
                for _ in 0..100 {
                    if lim.check("1.2.3.4").is_ok() {
                        ok += 1;
                    }
                }
                ok
            }));
        }

        let total_ok: u32 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        // Total successful requests should be exactly 500 (max_requests)
        assert_eq!(total_ok, 500);

        // Final count should be 500
        let entry = limiter.requests.get("1.2.3.4").unwrap();
        assert_eq!(entry.0, 500);
    }

    // ── 持久化测试（P2-15） ───────────────────────────────────

    #[test]
    fn test_persist_and_load_roundtrip() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let config = RateLimitConfig {
            enabled: true,
            max_requests: 60,
            window_seconds: 60,
            trust_forwarded_for: false,
        };
        let limiter = RateLimiter::new(config.clone());

        // Add some entries
        limiter.check("1.1.1.1");
        limiter.check("1.1.1.1");
        limiter.check("2.2.2.2");

        // Persist
        limiter.persist_to_db(&db);

        // Create a new limiter and load from DB
        let limiter2 = RateLimiter::new(config);
        limiter2.load_from_db(&db);

        // Verify state was restored
        let e1 = limiter2.requests.get("1.1.1.1").unwrap();
        assert_eq!(e1.0, 2);

        let e2 = limiter2.requests.get("2.2.2.2").unwrap();
        assert_eq!(e2.0, 1);
    }

    #[test]
    fn test_load_skips_expired_entries() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        // Insert an expired entry directly into the database
        let past_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - 120; // 2 minutes ago (expired for 60s window)
        db.save_rate_limit_state(&[
            ("expired.ip".to_string(), 5u32, past_time),
            ("valid.ip".to_string(), 3u32, SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64),
        ])
        .unwrap();

        let config = RateLimitConfig {
            enabled: true,
            max_requests: 60,
            window_seconds: 60,
            trust_forwarded_for: false,
        };
        let limiter = RateLimiter::new(config);
        limiter.load_from_db(&db);

        // Expired entry should not be loaded
        assert!(limiter.requests.get("expired.ip").is_none());
        // Valid entry should be loaded
        assert!(limiter.requests.get("valid.ip").is_some());
    }

    #[test]
    fn test_persist_skips_expired() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.initialize().unwrap();

        let config = RateLimitConfig {
            enabled: true,
            max_requests: 60,
            window_seconds: 1, // 1 second window for testing
            trust_forwarded_for: false,
        };
        let limiter = RateLimiter::new(config);

        limiter.check("1.1.1.1");
        limiter.check("2.2.2.2");

        // Wait for entries to expire
        std::thread::sleep(Duration::from_millis(1100));

        // Persist should skip expired entries
        limiter.persist_to_db(&db);

        // Database should be empty (all entries were expired)
        let state = db.load_rate_limit_state().unwrap();
        assert!(state.is_empty());
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
