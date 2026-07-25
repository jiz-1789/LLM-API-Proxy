# LLM API Proxy - 技术设计文档 v1.0

## 一、目标
基于 PRD，提供完整的系统架构、数据模型、接口契约和核心算法。实现语言推荐 **Rust + Tauri**（兼顾性能与桌面 GUI）。

---

## 二、系统架构总览

```
+------------------+     /v1/chat/completions      +-----------------------+
|  AI Chat Client  | ---------------------------> |  Local Proxy Gateway  |
|  (Bash/IDE/...)  |                              |  127.0.0.1:47339      |
+------------------+                              +-----------------------+
                                                    |  Pool Router          |
                                                    |  SSE Optimizer        |
                                                    |  Circuit Breaker      |
                                                    +-----------+-----------+
                                                                |
             +------------------+    +------------------+       |
             | Upstream A       |<-->|  Round-Robin      |       |
             | 127.0.0.1:8080   |    |  + Failover List  |       |
             +------------------+    +------------------+       |
                                                             |
             +------------------+    +------------------+     |
             | Upstream B       |<-->|  Ordered SSE     |     |
             | grok-4.5         |    |  Buffered Stream |     |
             +------------------+    +------------------+     |
```

### 部署形态
- **单文件 .exe**：编译后的 Tauri bundle 单文件，包含：
  - 后端二进制（Rust）
  - 内置前端资源（HTML/CSS/JS）
  - 运行时依赖（无需 Node.js/Docker）
- **本地存储目录**：`.data` 同级目录，仅含 `config.db`（SQLite）+ 日志。

---

## 三、数据模型（SQLite）

### 3.1 upstreams 表（上游 API）

```sql
CREATE TABLE IF NOT EXISTS upstreams (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,                  -- 'OpenAI-Proxy', 'DeepSeek-Dev'
    base_url TEXT NOT NULL,                     -- 'http://127.0.0.1:8080'
    default_api_key TEXT NOT NULL,              -- AES-256-GCM 加密
    api_key_master_id INTEGER NOT NULL REFERENCES master_keys(id),
    is_active BOOLEAN DEFAULT 1,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### 3.2 pools 表（模型池）

```sql
CREATE TABLE IF NOT EXISTS pools (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    model_name TEXT NOT NULL UNIQUE,            -- 'grok-4.5'（用户侧看到的名称）
    round_robin INTEGER DEFAULT 1,              -- 轮询开关
    thinking_mode INTEGER DEFAULT 0,            -- 0: 禁用, 1: 启用
    max_concurrency INTEGER DEFAULT 10,         -- 最大并发数
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### 3.3 pool_upstreams 表（池-上游绑定）

```sql
CREATE TABLE IF NOT EXISTS pool_upstreams (
    pool_id INTEGER NOT NULL REFERENCES pools(id) ON DELETE CASCADE,
    upstream_id INTEGER NOT NULL REFERENCES upstreams(id) ON DELETE CASCADE,
    sort_order INTEGER DEFAULT 0,               -- 决定轮询顺序
    primary KEY (pool_id, upstream_id)
);
```

### 3.4 requests 表（请求流水）

```sql
CREATE TABLE IF NOT EXISTS requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_uuid TEXT NOT NULL UNIQUE,          -- 请求级 UUID
    pool_id INTEGER REFERENCES pools(id),
    model_name TEXT NOT NULL,
    upstream_id INTEGER NOT NULL,
    upstream_url TEXT NOT NULL,                 -- 实际调用的上游地址
    success BOOLEAN DEFAULT 0,
    http_status INTEGER,
    latency_ms INTEGER,
    error_message TEXT,                         -- 失败原因
    error_trace TEXT,                           -- 完整错误栈
    started_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    completed_at DATETIME,
    stream_chunks INTEGER DEFAULT 0             -- SSE chunk 数量
);
```

### 3.5 master_keys 表（AES 主密钥）

```sql
CREATE TABLE IF NOT EXISTS master_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key_value BLOB NOT NULL,                    -- 32 字节随机盐值
    salt BLOB NOT NULL,                         -- 16 字节 nonce
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

### 3.6 数据流示意
```
[配置写入] → SQLite INSERT/UPDATE → WAL 模式
[API Key 加密] → MasterKey 解密 → AES-256-GCM 加解密 → 内存使用（不落盘明文）
[请求日志] → INSERT → 自动清理策略（按日期分片或删除）
```

---

## 四、核心 API 契约

### 4.1 客户端请求（OpenAI 兼容）

```http
POST /v1/chat/completions
Host: 127.0.0.1:47339
Content-Type: application/json

{
  "model": "grok-4.5",          -- 匹配 pool.model_name
  "messages": [...],
  "stream": true,                -- SSE 支持
  "temperature": 0.7,
  "reasoning": false             -- 可选：强制关闭思考模式
}
```

### 4.2 管理端接口

| 方法 | 路径 | 描述 |
|------|------|------|
| POST | `/api/v1/upstreams` | 创建上游 |
| GET | `/api/v1/upstreams` | 列出所有上游 |
| PUT | `/api/v1/upstreams/:id` | 更新上游（重新加密） |
| DELETE | `/api/v1/upstreams/:id` | 删除上游 |
| POST | `/api/v1/pools` | 创建模型池 |
| GET | `/api/v1/pools` | 列出所有池 |
| POST | `/api/v1/pools/:id/upstreams` | 添加上游到池 |
| DELETE | `/api/v1/pools/:id/upstreams/:upstream_id` | 从池移除上游 |
| GET | `/api/v1/pools/:id/models` | 一键获取上游模型列表 |
| GET | `/api/v1/logs` | 查看请求日志 |
| GET | `/api/v1/settings` | 全局设置 |
| POST | `/api/v1/settings/master-key` | 设置/重置 AES 主密钥 |

---

## 五、路由与负载均衡算法

### 5.1 请求生命周期

```
1. 收到 /v1/chat/completions
2. 解析 model → 查找 pool
3. pool.disabled? → 404 Not Found
4. pool.thinking_mode=1 & client.reasoning=false? → 过滤 thinking 字段
5. 选择 upstream via round-robin（或按权重）
6. 检查 circuit_breaker（熔断器状态）
   - OPEN: 跳过该上游，尝试下一个
   - HALF_OPEN: 放行一个探测请求
   - CLOSED: 正常转发
7. 转发请求至 upstream（带 API Key + 参数注入）
8. 若 SSE → 优化 chunk 组装逻辑
9. 记录请求流水（成功/失败/延迟）
10. 若失败 → circuit_breaker 触发 OPEN，切换到下一个 upstream
```

### 5.2 Round-Robin 伪代码

```rust
fn select_upstream(pool: &Pool) -> Option<Upstream> {
    let candidates = pool.upstreams.filter(|u| !circuit_breaker.is_open(u.id));
    
    if candidates.is_empty() {
        return None; // 全部不可用
    }
    
    // 轮询索引（原子操作）
    let idx = next_index(pool.id);
    candidates[idx % candidates.len()]
}
```

### 5.3 并发控制与乱序保护

#### 方案一：连接级有序保证（推荐）
- 每个上游维护一个 **并发信号量**
- 当同一上游的并发数达到阈值时，新请求排队等待
- 保证输出顺序 = 输入顺序（无重排）
- 适用于：单上游高吞吐场景

```rust
struct UpstreamGuard {
    semaphore: Semaphore, // tokio::sync::Semaphore
}

async fn execute_with_order(upstream: &UpstreamGuard, request: Request) -> Response {
    let permit = upstream.semaphore.acquire().await?;
    let response = forward_request(request).await;
    drop(permit); // 释放信号量，允许下一个请求
    response
}
```

#### 方案二：SSE Chunk Buffering（备选）
- 对每个请求分配唯一 `request_id`
- 客户端收到的 chunk 携带 `request_id`
- 客户端按 `request_id` 分组重组

```http
// 上游原始 SSE
data: {"id":"req-1","choices":[{"delta":{"content":"Hello"}}]}
data: {"id":"req-1","choices":[{"delta":{"content":" World"}}]}

// Proxy 注入 request_id
data: {"__proxy_req_id":"abc-123","id":"req-1","choices":[...]}
data: {"__proxy_req_id":"abc-123","id":"req-1","choices":[...]}
```

---

## 六、SSE 响应优化机制

### 6.1 问题背景
- 原生上游 SSE chunk 可能较小（单字符/单词）
- 网络延迟导致 chunk 到达时间不连续
- 客户端 UI 需要流畅展示

### 6.2 优化策略

| 策略 | 说明 | 默认值 |
|------|------|--------|
| Chunk Coalescing | 合并小 chunk 为批次发送 | 5 chunks / 50ms |
| Idle Timeout | 空闲等待后 flush | 100ms |
| Ordered Delivery | 保证 chunk 顺序 = 上游输出顺序 | 强制 |

### 6.3 伪代码

```rust
async fn stream_response(mut proxy_rx:mpsc::Receiver<SSEChunk>, client_tx:sink) {
    let mut buffer: Vec<SSEChunk> = vec![];
    let mut idle_timer = tokio::time::interval(100ms);
    
    loop {
        tokio::select! {
            chunk = proxy_rx.recv() => {
                buffer.push(chunk);
                if buffer.len() >= 5 {
                    flush(&mut buffer, &client_tx).await;
                }
            }
            _ = idle_timer.tick() => {
                flush(&mut buffer, &client_tx).await;
            }
        }
    }
}
```

---

## 七、思考模式参数映射表

| Vendor | Thinking Parameter | Injection Logic |
|--------|-------------------|-----------------|
| DeepSeek | `reasoning: true` | 在 messages 前追加 system prompt 或 body 字段 |
| OpenAI | `reasoning_effort: "medium"` | 替换 body 中 `reasoning_effort` 字段 |
| Claude | `thinking: {"type": "enabled"}` | 加入 body 顶层 `thinking` 对象 |

### 7.1 过滤规则
- **客户端显式设置 `reasoning: false`** → 无论如何都移除思考参数
- **Pool 开启思考模式** → 自动注入对应 vendor 参数
- **客户端未指定** → 沿用 Pool 配置

---

## 八、API Key 加密与便携存储方案

### 8.1 主密钥生成
```bash
# 首次启动或手动设置时生成
openssl rand -hex 16 > master_salt.hex  # 16 bytes salt
openssl rand -hex 32 > master_key.hex  # 32 bytes key
```

### 8.2 加密流程
```
User Input API Key → AES-256-GCM(plaintext=key, aad=upstream_id, iv=nonce) → ciphertext
```

### 8.3 解密流程
```
SQLite ciphertext → MasterKey + Salt → AES-256-GCM decrypt → plaintext (内存使用，不落盘)
```

### 8.4 便携存储结构
```
LLM-API-Proxy/
├── llm-proxy.exe          # 主程序
├── .data/
│   ├── config.db           # SQLite 数据库（含加密密钥）
│   ├── logs/
│   │   └── 2026-07-25.log
│   └── tmp/                # 临时文件（清理）
└── README.md
```

### 8.5 密钥持久化
- **Master Key** 存储在 `master_keys.key`（明文，仅限本地读取权限）
- **API Keys** 加密后存 SQLite
- **建议**：设置文件权限为 `chmod 600`（Linux）或 Windows ACL

---

## 九、错误追踪机制

### 9.1 错误分级
| 级别 | 示例 | 处理 |
|------|------|------|
| WARN | 单个上游超时 | 切换到备用 upstream |
| ERROR | 连续失败 5 次 | 熔断该 upstream 30s |
| CRITICAL | 所有上游不可用 | 返回 503 给客户端 |

### 9.2 日志字段
```json
{
  "timestamp": "2026-07-25T14:31:00Z",
  "level": "ERROR",
  "module": "router",
  "message": "Upstream failed after 3 retries",
  "upstream": "OpenAI-Proxy",
  "pool": "grok-4.5",
  "error_code": "CONNECTION_TIMEOUT",
  "attempt": 3,
  "latency_ms": 5000,
  "stack_trace": "..."
}
```

### 9.3 故障切换日志
```
[INFO] Selecting upstream for pool=grok-4.5, strategy=round-robin
[DEBUG] Upstream OpenAI-Proxy available (circuit=CLOSED)
[DEBUG] Forwarding to http://127.0.0.1:8080/v1/chat/completions
[ERROR] Upstream OpenAI-Proxy failed: 401 Unauthorized
[WARN] Switching to backup: DeepSeek-Dev
[DEBUG] Selected DeepSeek-Dev for next attempt
```

---

## 十、实现步骤（建议）

### Phase 1: Core Gateway（第 1-2 周）
1. 搭建 Rust/Tauri 项目结构
2. 实现 SQLite 数据模型
3. 完成 Upstream 管理 API
4. 实现 Pool 管理和轮询路由
5. 基础 HTTP 转发（非流式）

### Phase 2: Streaming & Thinking（第 3-4 周）
6. 实现 SSE 流式转发
7. 开发 Chunk Coalescing 算法
8. 集成 Thinking Mode 参数映射
9. 完成管理端 GUI 界面

### Phase 3: Resilience & Polish（第 5-6 周）
10. 实现 Circuit Breaker
11. 完善错误追踪和日志
12. 测试单文件 exe 打包
13. 便携性验证（U 盘运行测试）

---

## 十一、关键决策记录

| 决策 | 理由 |
|------|------|
| 使用 Rust + Tauri | 性能好、体积小、原生 GUI |
| SQLite 而非 JSON | 并发安全、查询能力强 |
| AES-256-GCM | 认证加密，防篡改 |
| 连接级有序保证 | 简单可靠，无需客户端改造 |
| Chunk Coalescing | 平衡流畅度和内存占用 |
| 单文件 exe | 便携性需求，U 盘可用 |