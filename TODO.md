# TODO — P0 稳定性底座

> 基于《项目优化建议.md》制定，技术方案见对话记录。
> 当前分支：`feature/p0-stability`

---

## 分支与合并策略（强制）

```
main（稳定发布，严禁直接开发）
  │
  ├── feature/p0-stability    ← P0 所有开发在此分支进行
  │     │
  │     └── 每个阶段完成后可推送到 beta 分支组合测试
  │
  └── beta                    ← 组合测试分支
        │
        └── 测试通过 + 用户确认后 → 合并到 main
```

**核心规则：**

- ✅ 所有 P0 开发在 `feature/p0-stability` 分支进行
- ✅ 每个阶段完成后，可合并到 `beta` 分支进行组合测试
- ✅ `beta` 分支测试无 bug + **用户明确确认**后，才能合并到 `main`
- ❌ 未经用户同意，**严禁合并到 main**
- ❌ 严禁在 `main` 分支上进行任何开发工作
- ❌ 严禁跳过 beta 测试直接合并到 main

---

## 第一阶段：限流重构 + 错误模型（可并行）✅ 已完成

> Commit `0d4901e` — `refactor(gateway): 限流模块重构、客户端IP识别、统一上游错误模型与标准化错误响应`
> 8 files changed, +1250 / -156，新增 35 个单元测试，全部 94 个测试通过。

### P0-1 限流模块重构为可配置独立模块 ✅
- [x] 新建 `src/gateway/rate_limit.rs`，将 `RateLimiter` 从 `gateway/mod.rs` 抽出
- [x] 新增 `RateLimitConfig` 结构体（enabled / max_requests / window_seconds / trust_forwarded_for）
- [x] 配置从 `settings` 表读取，支持运行时修改（重启生效）
- [x] `GatewayState` 中 `rate_limiter` 类型改为新模块
- [x] 429 响应增加 `Retry-After` 头
- [x] 单元测试（9 个）：窗口过期重置、超限拒绝、配置开关、独立 IP 计数、过期清理

### P0-2 明确客户端 IP 识别策略 ✅
- [x] 实现 `extract_client_ip()` 函数，区分直连 / 反向代理模式
- [x] 直连模式：使用 `remote_addr`
- [x] 反向代理模式：取 `X-Forwarded-For` 最右侧 IP（`next_back()`）
- [x] `trust_forwarded_for` 配置项控制模式切换
- [x] 单元测试（8 个）：直连无 XFF、直连忽略 XFF、代理单跳、代理多跳、代理无 XFF 回退、空 XFF、无 remote_addr

### P0-3 建立统一上游错误模型 ✅
- [x] 新建 `src/proxy/error.rs`，定义 `UpstreamError` 枚举
- [x] 错误分类：ConnectionFailed / Timeout / AuthFailed / ClientError / ServerError / ResponseFormatError / EmbeddedError / KeyDecryptionFailed
- [x] 定义 `TimeoutPhase`：Connect / ResponseHeaders / ResponseBody
- [x] 实现 `should_failover()` 方法（4xx 不触发故障转移）
- [x] 改造 `failover.rs`：用 `UpstreamError` 替换 `AppError::UpstreamFailed(String)`
- [x] 网关故障转移循环使用 `should_failover()` 决定是否继续
- [x] 单元测试（17 个）：各错误类型的 should_failover 判定、from_http_status 映射、error_summary、Display

### P0-4 OpenAI 兼容错误响应标准化映射 ✅
- [x] 新建 `src/gateway/error_response.rs`，统一构建 OpenAI 兼容错误响应
- [x] 所有 handler 中的错误响应改为调用 `error_response()` 系列函数
- [x] 错误响应格式：`{"error": {"message", "type", "code", "param": null}}`
- [x] 错误响应头携带 `X-Request-Id` → 已在 P0-5 实现
- [x] 错误类型映射表：
  - [x] 401 → authentication_error / invalid_api_key
  - [x] 400 → invalid_request_error / missing_model
  - [x] 404 → invalid_request_error / model_not_found
  - [x] 429 → rate_limit_error / rate_limit_exceeded
  - [x] 502 → api_error / all_upstreams_failed
  - [x] 503 → api_error / all_upstreams_disabled 或 no_available_upstream
  - [x] 500 → api_error / internal_error

---

## 第二阶段：追踪 ID + 响应头 + 超时（依赖第一阶段）✅ 已完成

> Commit `3286b30` — `feat(gateway): 添加请求追踪ID、响应头透传与统一超时策略`
> 3 files changed，新增 11 个单元测试，全部 105 个测试通过。

### P0-5 添加请求级追踪 ID ✅
- [x] 在 `handle_chat_completions` 开头生成 `trace_id`（UUID v4 simple）
- [x] 透传到上游请求头 `X-Request-Id`（通过 `build_trace_headers()` 构建）
- [x] 所有响应（成功 + 失败）返回 `X-Request-Id` 头（通过 `with_request_id()` 统一注入）
- [x] `tracing` 日志使用结构化字段（trace_id, model, pool, provider, error 等）
- [x] `request_id` 复用 `trace_id`（格式 `req_{trace_id}`），数据库 `request_logs` 无需迁移
- [x] 单元测试（4 个）：`with_request_id` 注入、保留已有头、`build_trace_headers` 构建、空字符串边界

### P0-6 规范上游响应头透传策略 ✅
- [x] 定义响应头白名单前缀常量 `PASSTHROUGH_HEADER_PREFIXES`
- [x] 透传头前缀：`x-ratelimit-`、`openai-`、`anthropic-`
- [x] 实现 `filter_passthrough_headers()` 函数，过滤非白名单头
- [x] 非流式成功响应带上透传头 + `X-Request-Id`
- [x] 单元测试（7 个）：x-ratelimit-* 透传、openai-* 透传、anthropic-* 透传、非白名单过滤、空 headers、混合 headers、前缀完整性

### P0-7 统一流式和非流式请求的超时策略 ✅
- [x] 非流式：`timeout_secs` = `pool.timeout_seconds`（已实现于第一阶段）
- [x] 流式首字节：`timeout_secs` = `pool.timeout_seconds`（移除硬编码 60s，改用参数传入）
- [x] 流式 chunk 间空闲超时：默认 120s（`DEFAULT_STREAM_IDLE_TIMEOUT_SECS`），用 `tokio::time::timeout` 包裹 `next_line()`
- [x] `forward_stream_request` 接受 `timeout_secs` 参数，移除硬编码 60s
- [x] 超时正确终止流并记录日志（`warn!` with `trace_id` + `idle_timeout_secs`）
- [x] 两个 forward 函数统一接受 `extra_headers` 参数用于注入 `X-Request-Id`

---

## 第三阶段：健康检查 + 主动探测 + 状态扩展（独立）

### P0-8 重构健康检查为三层
- [ ] 新建 `src/gateway/health.rs`
- [ ] 应用健康：返回运行状态 + uptime
- [ ] 数据库健康：执行 `get_stats()` 验证连通性
- [ ] 上游健康：聚合统计 healthy/degraded/down 数量
- [ ] `/api/health` 返回三层合并 JSON
- [ ] 整体状态：app + db 均 ok → "ok"，否则 → "degraded"

### P0-9 实现上游主动探测机制和恢复窗口策略
- [ ] 在 `initialize_backend` 中启动后台探测 tokio task
- [ ] 探测间隔可配置（默认 300s，最小 60s）
- [ ] 探测逻辑复用 `do_health_check` 函数
- [ ] 探测结果更新上游 status（healthy / degraded / down）
- [ ] 恢复窗口：连续失败 ≥ threshold → down；down 状态只有探测成功才恢复
- [ ] 配置项：`probe_enabled`（默认 false）、`probe_interval_seconds`（默认 300）
- [ ] 未开启探测时保持现有行为（每次请求都尝试所有上游）

### P0-10 扩展上游状态记录
- [ ] 数据库迁移 v6：新增 3 个可空列
  - [ ] `upstreams.last_success_time TEXT`
  - [ ] `upstreams.last_error_reason TEXT`
  - [ ] `upstreams.recovered_at TEXT`
- [ ] 更新 `DEVELOPMENT.md` Schema 表和迁移历史
- [ ] 更新 `Upstream` 结构体 + `map_upstream_row` 列索引
- [ ] 新增 `update_upstream_health()` 方法
- [ ] 网关 handler 中每次上游成功 / 失败都更新状态
- [ ] 单元测试：新字段读写、NULL 兼容

---

## 第四阶段：集成测试（依赖前面全部完成）

### P0-11 建立网关核心链路集成测试
- [ ] 新增 dev-dependency：`wiremock`
- [ ] 测试：有效 API Key + 正确 model → 成功转发 + model 替换
- [ ] 测试：无效 API Key → 401
- [ ] 测试：缺少 model 字段 → 400
- [ ] 测试：未知 model → 404
- [ ] 测试：第一个上游 5xx → 故障转移到第二个上游
- [ ] 测试：所有上游失败 → 502
- [ ] 测试：所有上游禁用 → 503
- [ ] 测试：请求日志正确记录到数据库
- [ ] 测试：4xx 不触发故障转移

### P0-12 建立流式代理模拟上游测试
- [ ] 测试：SSE 流式透传 + model 字段替换
- [ ] 测试：流式响应中 `error` 字段检测
- [ ] 测试：流式 token 用量提取（stream_options.include_usage）
- [ ] 测试：流式首字节超时
- [ ] 测试：流式空闲超时（chunk 间隔过长）
- [ ] 测试：`data: [DONE]` 正确处理

---

## 提交计划

> 所有 commit 在 `feature/p0-stability` 分支上提交。
> 每个阶段完成后可推送至 `beta` 分支组合测试，测试通过 + 用户确认后才能合并到 `main`。

| 顺序 | Commit | 内容 | 状态 | 合并到 beta |
|------|--------|------|------|-------------|
| 1 | `refactor(gateway): 限流模块重构、客户端IP识别、统一上游错误模型与标准化错误响应` | P0-1 + P0-2 + P0-3 + P0-4 | ✅ 已完成 | 待合并 |
| 2 | `feat(gateway): 添加请求追踪ID、响应头透传与统一超时策略` | P0-5 + P0-6 + P0-7 | ✅ 已完成 | 待合并 |
| 3 | `feat(gateway): 三层健康检查、主动探测与上游状态扩展` | P0-8 + P0-9 + P0-10 | 待开发 | 第三阶段完成后 |
| 4 | `test(gateway): 网关核心链路与流式代理集成测试` | P0-11 + P0-12 | 待开发 | 第四阶段完成后 |

总计：约 28-30 小时，4 个 commit（原计划 5 个，P0-1~P0-4 合并为 1 个）。

### beta 测试检查点

每个阶段合并到 `beta` 后，需验证以下内容：

- [x] `cargo build` 编译通过（第一、二阶段已验证）
- [x] `cargo clippy` 新增代码无 WARNING（第一、二阶段已验证）
- [x] `cargo test` 全部通过 — 105 passed; 0 failed（第二阶段已验证）
- [ ] 手动测试：网关基本功能正常（认证、转发、流式、故障转移）
- [ ] 手动测试：新功能按预期工作（限流配置、错误响应格式）
- [ ] 数据库迁移在旧数据库上升级无报错

**全部通过后，等待用户确认才能合并到 `main`。**

---

## 数据库变更汇总

### 迁移 v6（P0-10）

```sql
ALTER TABLE upstreams ADD COLUMN last_success_time TEXT;
ALTER TABLE upstreams ADD COLUMN last_error_reason TEXT;
ALTER TABLE upstreams ADD COLUMN recovered_at TEXT;
```

三列均可空（NULL），完全兼容旧数据。

### 新增 settings 键（无需迁移）

| key | 默认值 | 任务 | 状态 |
|-----|--------|------|------|
| `rate_limit_enabled` | `true` | P0-1 | ✅ 已实现 |
| `rate_limit_max_requests` | `60` | P0-1 | ✅ 已实现 |
| `rate_limit_window_seconds` | `60` | P0-1 | ✅ 已实现 |
| `rate_limit_trust_xff` | `false` | P0-2 | ✅ 已实现 |
| `probe_enabled` | `false` | P0-9 | 待开发 |
| `probe_interval_seconds` | `300` | P0-9 | 待开发 |

---

## 新增文件清单

```
src/gateway/rate_limit.rs       ← 限流模块                    ✅ 已创建
src/gateway/error_response.rs   ← 统一错误响应                ✅ 已创建
src/proxy/error.rs              ← 上游错误模型                ✅ 已创建
src/gateway/health.rs           ← 三层健康检查                待创建
src/gateway/tests.rs            ← 集成测试                    待创建
```

## 修改文件清单

```
src/gateway/mod.rs              ← 瘦身：移除内联限流，handler 错误响应统一化  ✅ 已完成
src/proxy/failover.rs           ← 错误分类 + 响应头透传 + 超时统一            ✅ 已完成
src/proxy/mod.rs                ← 声明新模块                                   ✅ 已完成
src/lib.rs                      ← 加载限流配置                                 ✅ 已完成
src/error.rs                    ← AppError 调整                                待开发（P0-3 已通过 From 兼容）
src/db.rs                       ← 迁移 v6 + Upstream 结构体 + update_upstream_health()  待开发
src/config.rs                   ← 新增限流/探测配置项                          待开发
DEVELOPMENT.md                  ← Schema 表 + 迁移历史更新                     待开发
dist/index.html                 ← 设置页新增限流/探测配置区域                  待开发
```
