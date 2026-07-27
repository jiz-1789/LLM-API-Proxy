# TODO — P0 稳定性底座

> 基于《项目优化建议.md》制定，技术方案见对话记录。
> 当前分支：`feature/rate-limit`

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

## 第一阶段：限流重构 + 错误模型（可并行）

### P0-1 限流模块重构为可配置独立模块
- [ ] 新建 `src/gateway/rate_limit.rs`，将 `RateLimiter` 从 `gateway/mod.rs` 抽出
- [ ] 新增 `RateLimitConfig` 结构体（enabled / max_requests / window_seconds / trust_forwarded_for）
- [ ] 配置从 `settings` 表读取，支持运行时修改
- [ ] `GatewayState` 中 `rate_limiter` 类型改为新模块
- [ ] 429 响应增加 `Retry-After` 头
- [ ] 单元测试：窗口过期重置、超限拒绝、配置开关

### P0-2 明确客户端 IP 识别策略
- [ ] 实现 `extract_client_ip()` 函数，区分直连 / 反向代理模式
- [ ] 直连模式：使用 `remote_addr`
- [ ] 反向代理模式：取 `X-Forwarded-For` 最右侧 IP
- [ ] `trust_forwarded_for` 配置项控制模式切换
- [ ] 单元测试：直连、XFF 存在、XFF 不存在、多级代理

### P0-3 建立统一上游错误模型
- [ ] 新建 `src/proxy/error.rs`，定义 `UpstreamError` 枚举
- [ ] 错误分类：ConnectionFailed / Timeout / AuthFailed / ClientError / ServerError / ResponseFormatError / EmbeddedError
- [ ] 定义 `TimeoutPhase`：Connect / ResponseHeaders / ResponseBody
- [ ] 实现 `should_failover()` 方法（4xx 不触发故障转移）
- [ ] 改造 `failover.rs`：用 `UpstreamError` 替换 `AppError::UpstreamFailed(String)`
- [ ] 单元测试：各错误类型的 should_failover 判定

### P0-4 OpenAI 兼容错误响应标准化映射
- [ ] 新建 `src/gateway/error_response.rs`，统一构建 OpenAI 兼容错误响应
- [ ] 所有 handler 中的错误响应改为调用 `error_response()`
- [ ] 错误响应格式：`{"error": {"message", "type", "code", "param": null}}`
- [ ] 错误响应头携带 `X-Request-Id`
- [ ] 错误类型映射表：
  - [ ] 401 → authentication_error / invalid_api_key
  - [ ] 400 → invalid_request_error / missing_model
  - [ ] 404 → invalid_request_error / model_not_found
  - [ ] 429 → rate_limit_error / rate_limit_exceeded
  - [ ] 502 → api_error / all_upstreams_failed
  - [ ] 503 → api_error / all_upstreams_disabled 或 no_available_upstream
  - [ ] 500 → api_error / internal_error

---

## 第二阶段：追踪 ID + 响应头 + 超时（依赖第一阶段）

### P0-5 添加请求级追踪 ID
- [ ] 在 `handle_chat_completions` 开头生成 `trace_id`（`trace_` + UUID v4）
- [ ] 透传到上游请求头 `X-Request-Id`
- [ ] 所有响应（成功 + 失败）返回 `X-Request-Id` 头
- [ ] `tracing` 日志使用结构化字段（trace_id, request_id, model, pool）
- [ ] `failed_upstreams` JSON 包含 trace_id 便于关联
- [ ] 数据库 `request_logs` 已有 `request_id` 字段，无需迁移

### P0-6 规范上游响应头透传策略
- [ ] 定义响应头白名单常量 `PASSTHROUGH_HEADERS`
- [ ] 透传头：x-ratelimit-*、openai-* 系列
- [ ] 在 `failover.rs` 的 `Response` 中实际填充透传头（替换当前空 HeaderMap）
- [ ] 非流式成功响应带上透传头 + `X-Request-Id`
- [ ] 单元测试：白名单头透传、非白名单头过滤

### P0-7 统一流式和非流式请求的超时策略
- [ ] 定义 `TimeoutConfig`：connect / response / stream_first_byte / stream_idle
- [ ] 非流式：`response_timeout` = `pool.timeout_seconds`
- [ ] 流式首字节：`stream_first_byte_timeout` = `pool.timeout_seconds`
- [ ] 流式 chunk 间空闲超时：默认 120s，用 `tokio::time::timeout` 包裹 `next_line()`
- [ ] 移除 `forward_stream_request` 中硬编码的 60s
- [ ] 超时返回 `UpstreamError::Timeout { phase }`
- [ ] 单元测试：非流式超时、流式首字节超时、流式空闲超时

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

| 顺序 | Commit | 内容 | 预估 | 合并到 beta |
|------|--------|------|------|-------------|
| 1 | `refactor(gateway): 抽离限流模块为独立可配置组件` | P0-1 + P0-2 | ~4h | 第一阶段完成后 |
| 2 | `refactor(proxy): 建立统一上游错误模型与标准化错误响应` | P0-3 + P0-4 | ~5h | 同上 |
| 3 | `feat(gateway): 添加请求追踪ID、响应头透传与统一超时策略` | P0-5 + P0-6 + P0-7 | ~5h | 第二阶段完成后 |
| 4 | `feat(gateway): 三层健康检查、主动探测与上游状态扩展` | P0-8 + P0-9 + P0-10 | ~7h | 第三阶段完成后 |
| 5 | `test(gateway): 网关核心链路与流式代理集成测试` | P0-11 + P0-12 | ~7h | 第四阶段完成后 |

总计：约 28-30 小时，5 个 commit。

### beta 测试检查点

每个阶段合并到 `beta` 后，需验证以下内容：

- [ ] `cargo build` 编译通过
- [ ] `cargo clippy` 无 WARNING / ERROR
- [ ] `cargo test` 全部通过
- [ ] 手动测试：网关基本功能正常（认证、转发、流式、故障转移）
- [ ] 手动测试：新功能按预期工作
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

| key | 默认值 | 任务 |
|-----|--------|------|
| `rate_limit_enabled` | `true` | P0-1 |
| `rate_limit_max_requests` | `60` | P0-1 |
| `rate_limit_window_seconds` | `60` | P0-1 |
| `rate_limit_trust_xff` | `false` | P0-2 |
| `probe_enabled` | `false` | P0-9 |
| `probe_interval_seconds` | `300` | P0-9 |

---

## 新增文件清单

```
src/gateway/rate_limit.rs       ← 限流模块
src/gateway/error_response.rs   ← 统一错误响应
src/gateway/health.rs           ← 三层健康检查
src/proxy/error.rs              ← 上游错误模型
src/gateway/tests.rs            ← 集成测试
```

## 修改文件清单

```
src/error.rs                    ← AppError 调整（UpstreamFailed 改为包裹 UpstreamError）
src/gateway/mod.rs              ← 瘦身：移除内联限流，handler 错误响应统一化
src/proxy/failover.rs           ← 错误分类 + 响应头透传 + 超时统一
src/proxy/mod.rs                ← 声明新模块
src/db.rs                       ← 迁移 v6 + Upstream 结构体 + update_upstream_health()
src/lib.rs                      ← 启动主动探测后台任务
src/config.rs                   ← 新增限流/探测配置项
DEVELOPMENT.md                  ← Schema 表 + 迁移历史更新
dist/index.html                 ← 设置页新增限流/探测配置区域
```
