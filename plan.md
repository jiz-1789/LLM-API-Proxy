# LLM-API-Proxy 开发计划

> **最后更新**：2026-07-25  
> **技术栈**：Rust + Tauri + SQLite  
> **目标**：8 周完成 MVP，交付单文件 `.exe` 便携桌面应用

---

## 一、项目目录结构

```
LLM-API-Proxy/
├── Cargo.toml                        # workspace 根配置
├── tauri.conf.json                   # Tauri 窗口/构建配置
├── build.rs                          # Tauri 构建脚本
│
├── src/                              # Rust 后端
│   ├── main.rs                       # 入口：Tauri 启动 + API 服务
│   ├── lib.rs                        # 模块声明
│   │
│   ├── gateway/                      # OpenAI 兼容网关
│   │   ├── mod.rs
│   │   ├── router.rs                 # 路由：/v1/models, /v1/chat/completions
│   │   ├── stream.rs                 # SSE 流式透传 + Chunk Coalescing
│   │   └── auth.rs                   # Gateway API Key 校验
│   │
│   ├── pool/                         # 号池与轮询逻辑
│   │   ├── mod.rs
│   │   ├── round_robin.rs            # 顺序轮询
│   │   ├── circuit_breaker.rs        # 熔断器状态机
│   │   ├── concurrency.rs            # 并发控制信号量
│   │   └── thinking.rs               # 思考模式参数注入
│   │
│   ├── proxy/                        # 上游转发
│   │   ├── mod.rs
│   │   ├── client.rs                 # HTTP Client (reqwest)
│   │   ├── failover.rs               # 故障转移链
│   │   └── model_filter.rs           # 模型名替换
│   │
│   ├── db/                           # 数据层
│   │   ├── mod.rs
│   │   ├── schema.rs                 # SQL DDL
│   │   ├── upstream.rs               # Upstream CRUD
│   │   ├── pool.rs                   # Pool CRUD
│   │   ├── settings.rs               # Gateway Settings
│   │   ├── request_log.rs            # 请求日志写入/查询
│   │   └── migration.rs              # 数据库迁移版本管理
│   │
│   ├── crypto/                       # 加密存储
│   │   ├── mod.rs
│   │   └── key_management.rs         # Master Key 生成/存储 + AES-256-GCM
│   │
│   ├── api/                          # 内部管理 API（GUI 调用）
│   │   ├── mod.rs
│   │   ├── upstreams.rs
│   │   ├── pools.rs
│   │   ├── settings.rs
│   │   ├── logs.rs
│   │   └── health.rs
│   │
│   ├── gui/                          # Tauri 后端命令（GUI 事件绑定）
│   │   ├── mod.rs
│   │   ├── dashboard.rs
│   │   └── event.rs                  # 事件发布/订阅
│   │
│   └── error.rs                      # 统一错误类型
│
├── src-tauri/                        # Tauri 前端资源
│   ├── icons/                        # 应用图标
│   └── capabilities/                 # 权限配置
│
├── src/frontend/                     # GUI 前端（嵌入 Tauri Webview）
│   ├── index.html                    # 入口页面
│   ├── css/
│   │   └── app.css
│   ├── js/
│   │   ├── app.js                    # 主应用逻辑
│   │   ├── pages/
│   │   │   ├── dashboard.js
│   │   │   ├── upstreams.js
│   │   │   ├── pools.js
│   │   │   ├── settings.js
│   │   │   ├── logs.js
│   │   │   └── health.js
│   │   └── components/
│   │       ├── sidebar.js
│   │       ├── modal.js
│   │       └── toast.js
│   └── api/
│       └── http.js                   # Tauri invoke 调用封装
│
├── data/                             # 运行时数据（随 exe 目录移动）
│   ├── proxy.db                      # SQLite
│   ├── master_key.bin                # AES 主密钥
│   └── logs/                         # 文本日志
│
├── .gitignore
├── README.md
├── LICENSE
└── plan.md                           # 本文件
```

---

## 二、阶段规划

### 🏗️ M0 — 工程骨架搭建（第 1 周，~40h）

建立完整可编译运行的 Rust + Tauri 项目骨架。此阶段**不实现业务逻辑**，只做基础设施。

| # | 任务 | 预估 | 验收标准 |
|---|------|------|----------|
| 1.1 | 初始化 `Cargo.toml` workspace（`app`, `gateway`, `db`, `crypto` crate） | 3h | `cargo build` 通过，输出 `LLM-API-Proxy.exe` |
| 1.2 | 创建 Tauri 项目脚手架（窗口、托盘、最小化到托盘） | 4h | 程序启动打开原生窗口，最小化后进程仍在托盘 |
| 1.3 | 搭建数据目录结构（`data/`），实现 Master Key 首次生成 | 3h | 首次运行自动在 `data/master_key.bin` 生成 32 字节随机密钥 |
| 1.4 | 编写 SQLite schema（全部表 DDL）并初始化空库 | 4h | `proxy.db` 创建成功，6 张表结构与设计文档一致 |
| 1.5 | 实现 AES-256-GCM 加解密模块（Master Key + API Key） | 4h | 测试用例：encrypt → decrypt 返回原文；Master Key 缺失时正确报错 |
| 1.6 | 实现统一错误类型 `error.rs` + 日志初始化（tracing/tracing-subscriber） | 3h | 所有业务模块使用统一 Error 类型，日志输出到 `data/logs/` |
| 1.7 | 实现内部 HTTP 服务器框架（axum/actix-web），注册健康端点 `/api/health` | 5h | 端口可配置，GET `/api/health` 返回 `{"status":"ok"}` |
| 1.8 | 编写 GitHub Actions CI 配置（Linux + Windows 交叉编译） | 4h | PR 触发后自动构建，Windows 输出 exe |
| 1.9 | 编写 README.md + 贡献指南 | 4h | 包含项目说明、构建步骤、部署方式 |
| 1.10 | 集成测试：完整启动流程 smoke test | 4h | 自动测试脚本验证启动→建库→健康检查→关闭 |

**Commit 策略**：每完成 1 个任务提交一次 commit，至少 10 次独立 commit。

**M0 结束时 Commit 示例**：
```
commit a1b2c3d feat: initialize Cargo workspace and Tauri scaffold
commit d4e5f6g feat(db): create SQLite schema with all tables
commit h7i8j9k feat(crypto): implement AES-256-GCM key management
commit l0m1n2o feat(gateway): add health endpoint
...
```

---

### 💾 M1 — 数据层 & 加密存储（第 2 周，~32h）

让数据库和加密系统可独立工作，为后续业务层提供稳定的 CRUD 接口。

| # | 任务 | 预估 | 验收标准 |
|---|------|------|----------|
| 2.1 | 实现 `upstreams` 表增删改查 | 5h | 能创建/查询/更新/删除上游订阅记录 |
| 2.2 | 实现 `pools` 表增删改查 | 4h | 支持唯一名称约束，创建/读取/修改/删除正常 |
| 2.3 | 实现 `pool_upstreams` 关联表操作 | 4h | 关联/解绑/按排序取上游列表 |
| 2.4 | 实现 `request_logs` 表写入和分页查询 | 3h | 日志写入无阻塞，查询支持时间范围过滤 |
| 2.5 | 实现 `settings` 配置读写 | 3h | 监听地址/端口/Gateway Key 可持久化 |
| 2.6 | API Key 加解密完整流程（存入前加密，使用时解密，内存不留明文） | 4h | 测试：保存→重启→解密→使用原 Key |
| 2.7 | 实现 Master Key 文件权限检查（Windows ACL 提示） | 3h | 运行时检测文件是否可被其他用户读取 |
| 2.8 | 数据库迁移框架（版本号 + upgrade 函数） | 3h | 新库自动建表，旧库升级无数据丢失 |
| 2.9 | 单元测试覆盖：全部 CRUD + 加密模块 | 5h | 覆盖率 ≥ 80%，CI 通过 |

**Commit 策略**：每个子模块完成后立即 commit，约 9 次独立 commit。

**M1 结束时 Commit 示例**：
```
commit e1f2g3h feat(db): upstream CRUD with encrypted API key
commit i4j5k6l feat(db): pool and pool_upstream association
commit m7n8o9p test(db): add unit tests for crypto module (80%+ coverage)
...
```

---

### 🔌 M2 — 上游管理 API & GUI 页面（第 3-4 周前半，~48h）

让用户能通过 GUI 添加和管理 API 上游。

| # | 任务 | 预估 | 验收标准 |
|---|------|------|----------|
| 3.1 | 实现 `/api/upstreams` GET/POST/PUT/DELETE/PATCH toggle | 4h | 5 个端点均可正常调用，响应格式符合设计文档 |
| 3.2 | 实现 `/api/upstreams/:id/fetch-models`（一键获取模型列表） | 4h | 输入 URL+Key 后调用上游 `/v1/models`，返回字符串列表 |
| 3.3 | GUI：上游管理页面布局（表格 + 新增/编辑弹窗） | 5h | 页面能展示所有上游，支持增删改查交互 |
| 3.4 | GUI：新增上游表单（供应商/Base URL/API Key/模型下拉/备注） | 4h | 表单验证完整，提交后调用后端 API |
| 3.5 | GUI：编辑上游弹窗 + 显示加密状态指示器 | 3h | 编辑不影响现有连接，API Key 字段半掩码显示 |
| 3.6 | GUI：一键获取模型列表按钮 + 加载状态 | 3h | 点击后显示 spinner，成功后填充下拉选项 |
| 3.7 | GUI：批量启用/禁用上游开关 | 3h | 开关切换即时生效，状态栏实时更新 |
| 3.8 | 端到端测试：添加/修改/删除一个完整上游 | 4h | 自动化 E2E 测试通过 |
| 3.9 | 性能测试：100 个上游记录的查询/更新 < 50ms | 3h | 满足预期性能 |
| 3.10 | 文档更新：API 接口文档 + 用户使用指南 | 3h | 设计文档与实际代码一致 |

**Commit 策略**：每个主要功能点（如 3.1-3.3）提交一次，约 10 次独立 commit。

**M2 结束时 Commit 示例**：
```
commit a2b3c4d feat(api): upstream CRUD endpoints
commit e5f6g7h feat(api): fetch upstream models endpoint
commit i8j9k0l feat(gui): upstream management page layout
commit m1n2o3p feat(gui): upstream add/edit form with validation
commit q4r5s6t feat(gui): fetch models button with loading state
...
```

---

### 🧠 M3 — 号池管理与轮询路由核心（第 4-5 周，~56h）

这是项目的核心引擎——决定请求如何分配给不同上游。

| # | 任务 | 预估 | 验收标准 |
|---|------|------|----------|
| 4.1 | 实现号池创建/编辑/删除 API + 关联上游 | 5h | 创建号池时选择多个上游，排序生效 |
| 4.2 | 实现 Round Robin 选择器（原子索引，线程安全） | 4h | 连续 N 次请求均匀分布在 N 条上游上 |
| 4.3 | 实现 Circuit Breaker 状态机（Healthy → Open → Half-Open → Closed） | 6h | 连续失败阈值触发熔断，超时后自动半开探测 |
| 4.4 | 实现故障转移链（按轮询顺序尝试下一条可用上游） | 5h | 第一条失败时自动尝试第二条，最终都失败则返回友好错误 |
| 4.5 | 实现并发信号量控制（每个号池独立限制并发数） | 5h | 超出最大并发的请求进入等待队列，释放后立即处理 |
| 4.6 | 实现连接级有序保证（单个上游同一时刻只接受一个请求，并发>1 时排队） | 6h | 并发测试：5 个同时请求，每个上游最多 1 个活跃请求 |
| 4.7 | 实现 Thinking Mode 参数注入（DeepSeek/OpenAI/Claude 三种厂商映射） | 5h | 开启思考模式后，发给上游的 JSON body 正确包含对应参数 |
| 4.8 | 实现请求追踪 ID（tracking_id 注入，检测乱序丢弃重路由） | 4h | tracking_id 在日志和上游请求头中可见 |
| 4.9 | GUI：号池管理页面（列表 + 创建/编辑弹窗 + 上游拖拽排序） | 6h | 页面无滚动溢出，拖拽排序后保存立即生效 |
| 4.10 | 单元测试：轮询分布性、熔断器状态转换、并发控制、故障转移 | 6h | 覆盖率 ≥ 90%，CI 通过 |

**Commit 策略**：每个核心组件单独 commit（如每个算法/逻辑单元），约 10 次独立 commit。

**M3 结束时 Commit 示例**：
```
commit b3c4d5e feat(pool): round-robin selector with atomic indexing
commit f6g7h8i feat(pool): circuit breaker state machine (Healthy→Open→Half-Open)
commit j9k0l1m feat(proxy): failover chain with friendly error fallback
commit n2o3p4q feat(proxy): concurrency semaphore per pool
commit r5s6t7u feat(proxy): connection-level ordering guarantee
commit v8w9x0y feat(thinking): inject reasoning parameters by vendor type
commit z1a2b3c feat(gui): pool management page with drag-and-drop reorder
commit d4e5f6g test(pool): comprehensive unit tests for router engine (90% coverage)
...
```

---

### 🌐 M4 — 统一网关 & SSE 流式代理（第 6 周，~40h）

让外部客户端能通过统一 URL 使用号池。

| # | 任务 | 预估 | 验收标准 |
|---|------|------|----------|
| 5.1 | 实现 `GET /v1/models`（返回所有号池名称） | 3h | 响应格式完全兼容 OpenAI `/v1/models` |
| 5.2 | 实现 `POST /v1/chat/completions`（非流式，转发到上游） | 5h | 发送 JSON → 接收完整 JSON → 模型名替换 → 返回客户端 |
| 5.3 | 实现 API Key 认证中间件（Gateway Key 校验） | 3h | 错误 Key 返回 401，正确 Key 放行 |
| 5.4 | 实现 SSE 流式透传（逐 chunk 回传，不解析/不修改） | 6h | 流式聊天体验流畅，chunk 顺序正确 |
| 5.5 | 实现 Chunk Coalescing（合并小 chunk，优化网络传输） | 4h | 默认 5 chunks / 50ms 批次发送 |
| 5.6 | 实现模型名替换（上游返回的 model 字段统一替换为号池名） | 3h | 无论上游返回什么模型名，客户端始终看到号池名 |
| 5.7 | 实现客户端断开即停止上游请求（cancel token） | 3h | 客户端断开后 upstream 请求立即中断 |
| 5.8 | 非流式请求性能优化（目标 ≤ 5ms 额外延迟） | 5h | Locust/k6 压测：100 QPS 下 P99 < 200ms |
| 5.9 | 集成测试：模拟真实 ChatGPT/Claude Desktop 客户端调用 | 5h | 通过 OpenAI SDK 完整对话测试 |
| 5.10 | 文档更新：客户端对接指南 + API 契约文档 | 3h | 用户可直接按文档配置第三方工具 |

**Commit 策略**：每个端点/核心机制单独 commit，约 10 次独立 commit。

**M4 结束时 Commit 示例**：
```
commit c5d6e7f feat(gateway): /v1/models endpoint
commit g8h9i0j feat(gateway): /v1/chat/completions non-streaming
commit k1l2m3n feat(gateway): Gateway API Key authentication middleware
commit o4p5q6r feat(stream): SSE passthrough with chunk coalescing
commit s7t8u9v feat(stream): streaming completion + cancel on disconnect
commit w0x1y2z3a perf(gateway): optimize proxy overhead to <5ms per request
commit b4c5d6e test(gateway): integration test with real OpenAI SDK client
...
```

---

### 🖥️ M5 — GUI 全功能 & 系统管理（第 7 周，~40h）

补全管理窗口所有功能模块，打造完整桌面应用体验。

| # | 任务 | 预估 | 验收标准 |
|---|------|------|----------|
| 6.1 | GUI：仪表盘页面（统计卡片：上游数/号池数/今日请求/资源占用） | 5h | 实时刷新，数据来自后端 API |
| 6.2 | GUI：网关设置页面（监听端口/API Key/日志级别） | 4h | 修改后热加载生效，无需重启 |
| 6.3 | GUI：请求日志页面（表格 + 时间筛选 + 错误溯源详情） | 5h | 显示最近请求，支持点击查看详情 |
| 6.4 | GUI：健康检查页面（一键测试所有上游连通性 + 状态灯） | 4h | 点击测试后逐条显示连通/超时/失败状态 |
| 6.5 | GUI：实时事件订阅（WebSocket 或 Tauri event，显示实时状态变化） | 6h | 新增上游/号池变更/请求失败时 UI 实时更新 |
| 6.6 | GUI：托盘图标右键菜单（打开窗口/暂停服务/退出） | 4h | 关闭窗口后最小化到托盘，托盘菜单功能正常 |
| 6.7 | GUI：主题切换（亮色/暗色模式） | 3h | 切换后持久化，重启保持 |
| 6.8 | 全局样式与设计系统（字体、间距、圆角、色彩规范） | 4h | 所有页面视觉一致，符合设计系统文档 |
| 6.9 | 端到端 GUI 测试（所有页面交互路径） | 4h | 自动化测试覆盖所有主要交互 |
| 6.10 | 文档更新：桌面 GUI 使用手册 | 3h | 图文并茂，新手可独立操作 |

**Commit 策略**：每个 GUI 模块/功能点独立 commit，约 10 次独立 commit。

**M5 结束时 Commit 示例**：
```
commit d7e8f9g feat(gui): dashboard page with live statistics
commit h0i1j2k feat(gui): gateway settings with hot-reload
commit l3m4n5o feat(gui): request logs with time filter and error details
commit p6q7r8s feat(gui): health check with status indicators
commit t9u0v1w feat(gui): real-time events via Tauri event bus
commit x2y3z4a feat(gui): system tray integration
commit b5c6d7e feat(gui): dark/light theme with persistence
commit f9g0h1i perf(gui): optimize re-render performance
commit j2k3l4m test(gui): E2E tests for all management pages
...
```

---

### 🚀 M6 — 性能调优 & 单文件打包部署（第 8 周，~32h）

打磨产品，使其达到生产可用水平。

| # | 任务 | 预估 | 验收标准 |
|---|------|------|----------|
| 7.1 | 性能压测：全链路 100 QPS 压力测试，P99 < 200ms | 4h | k6/Locust 报告达标 |
| 7.2 | 启动时间优化：冷启动 ≤ 3 秒 | 4h | 计时验证：双击 exe 到可接受请求 ≤ 3s |
| 7.3 | SQLite WAL 模式 + 查询优化（慢 SQL 分析） | 4h | 高频查询 < 10ms |
| 7.4 | Tauri bundle 打包为单文件 exe + 内置前端资源 | 5h | 输出的 exe 可独立运行，无需额外依赖 |
| 7.5 | U 盘便携模式验证（复制整个文件夹到另一台电脑可运行） | 4h | 跨电脑运行，Master Key 跟随数据目录 |
| 7.6 | 安全审计：API Key 内存清理、文件权限、XSS/CSRF 防护 | 5h | 无高危安全漏洞 |
| 7.7 | 版本信息 + 自动更新检查（预留接口，首期不调用） | 3h | exe 显示版本号，GUI 右上角可见 |
| 7.8 | 编写发布说明 + Changelog + 安装包/便携包 | 3h | v1.0.0 发布包可交付 |
| 7.9 | 最终回归测试（全部 PRD 功能点逐一验证） | 4h | PRD 9.1 所有必做项 ✅ |

**Commit 策略**：每个独立工作项 commit，约 9 次独立 commit。

**M6 结束时 Commit 示例**：
```
commit a1b2c3d perf: optimize SQLite queries to <10ms
commit e4f5g6h fix: secure memory cleanup for API keys
commit i7j8k9l build: package single-file exe with Tauri bundle
commit m0n1o2p test: full regression suite for PRD requirements
commit q3r4s5t docs: release notes and changelog for v1.0.0
...
```

---

## 三、Commit 提交规范

### 3.1 分支策略

```
main                          # 稳定版本，每次里程碑发布打 tag
├── feature/01-scaffold      # M0: 工程骨架
├── feature/02-database      # M1: 数据层
├── feature/03-upstream-mgr  # M2: 上游管理
├── feature/04-pool-router   # M3: 号池路由
├── feature/05-gateway-sse   # M4: 网关+SSE
├── feature/06-gui-full      # M5: GUI 全功能
├── feature/07-release       # M6: 性能+打包
```

### 3.2 Commit Message 格式

采用 Conventional Commits：

```
<type>(<scope>): <描述>

可选正文（BREAKING CHANGE 等）

- feat: 新功能
- fix: Bug 修复
- perf: 性能优化
- refactor: 重构
- test: 测试
- docs: 文档
- build: 构建/打包
- style: 样式
```

**示例**：

```
feat(pool): add circuit breaker with healthy/open/half-open states

- implements state machine for upstream health tracking
- configurable threshold (default 3 failures)
- auto-recovery after 60s cooldown

Closes #1
```

### 3.3 提交流程

```
1. 完成一个任务 → git add 相关文件
2. 写清晰的 commit message（说明做了什么、为什么）
3. git commit
4. git push origin <branch>
5. 在 PR/MR 中关联任务编号
```

### 3.4 Commit 粒度规则

| 原则 | 说明 |
|------|------|
| **一任务一 commit** | 每个任务完成后立即提交，不要在完成后一次性堆积所有改动 |
| **原子性** | 一个 commit 只做一件事，避免混合 commit |
| **可回滚** | 每个 commit 都应该是可独立 revert 的 |
| **有测试** | 业务逻辑相关 commit 必须包含对应测试 |
| **自包含** | 每个 commit 在 main 分支上可独立编译通过 |

---

## 四、验收标准

### 4.1 功能验收（对照 PRD 9.1 MVP）

| # | 功能点 | 状态 | 负责人 | 完成日期 |
|---|--------|------|--------|----------|
| 1 | 上游订阅 CRUD | ⬜ | | |
| 2 | 一键获取模型列表 | ⬜ | | |
| 3 | 号池创建与管理 | ⬜ | | |
| 4 | 统一 OpenAI 兼容入口 | ⬜ | | |
| 5 | 顺序轮询 | ⬜ | | |
| 6 | 基础故障转移 | ⬜ | | |
| 7 | 单页面 Dashboard | ⬜ | | |
| 8 | SQLite 持久化 | ⬜ | | |
| 9 | 请求日志查看 | ⬜ | | |
| 10 | 思考模式支持 | ⬜ | | |
| 11 | SSE 流式响应 | ⬜ | | |
| 12 | 单文件 exe 便携部署 | ⬜ | | |
| 13 | 并发数配置 | ⬜ | | |
| 14 | 请求有序性保障 | ⬜ | | |
| 15 | 错误溯源记录 | ⬜ | | |

### 4.2 非功能验收

| 指标 | 目标 | 测试方法 |
|------|------|----------|
| 单请求额外延迟 | ≤ 5ms | k6 压测，对比直连上游 |
| 并发支持 | ≥ 100 QPS | 多号池并行请求，监控错误率 < 1% |
| 启动时间 | ≤ 3 秒 | stopwatch 从双击 exe 到首个请求可处理 |
| API Key 加密 | AES-256-GCM | 内存 dump 无法找到明文 Key |
| 便携性 | U 盘跨电脑运行 | 复制到另一台 Windows 直接启动 |

### 4.3 代码质量要求

- **测试覆盖率**：核心业务逻辑 ≥ 80%，M3 路由引擎 ≥ 90%
- **Lint**：`rustfmt` + `clippy --fix` 通过
- **类型检查**：`cargo clippy` 无 WARN/ERROR
- **文档**：关键模块有 doc comments，公开 API 有 inline 文档

---

## 五、第二期迭代规划（MVP 之后）

| 功能 | 预估 | 说明 |
|------|------|------|
| 加权轮询 / 随机策略 | 1-2 周 | 基于响应速度/成功率动态调整权重 |
| Token 用量统计 | 1 周 | 记录每次请求的 prompt/completion token 数 |
| 配额限制 | 1 周 | 每个号池每日调用上限，超限时提醒 |
| HTTPS 支持 | 1 周 | 自签证书或 Let's Encrypt |
| 配置文件导出/导入 | 3 天 | JSON 备份/恢复，方便迁移 |
| Docker 部署 | 1 周 | 纯后端无 GUI 模式，适合服务器 |
| 多语言 UI | 1 周 | 中文/英文界面切换 |

---

## 六、第三期迭代规划

| 功能 | 预估 | 说明 |
|------|------|------|
| AI 自动推荐号池分组 | 2 周 | 根据使用频率和响应质量智能分组 |
| 多用户权限管理 | 2 周 | RBAC 角色体系 |
| 移动端管理 App | 3 周 | React Native / Flutter |
| 插件化上游适配器 | 2 周 | 允许社区开发非标格式适配 |
| Webhook 告警 | 1 周 | 上游故障/配额耗尽时推送通知 |

---

## 七、风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| Tauri 打包复杂度高 | 延期 | M0 预留充足时间，遇到问题提前寻求社区帮助 |
| SSE 流式乱序 | 用户体验差 | 严格实现连接级有序保证，单元测试覆盖并发场景 |
| 上游 API 格式偏差 | 转发失败 | 限定仅支持 OpenAI 兼容格式，记录非标准字段差异 |
| Master Key 丢失 | 数据不可恢复 | 备份 `master_key.bin`，文档中明确警告 |
| 跨平台兼容 | 部署受限 | MVP 聚焦 Windows，后续再扩展 macOS/Linux |
