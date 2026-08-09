# 更新日志

本项目所有重要变更均会记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [0.4.2] - 2026-08-09

### 修复
- 修复 OpenCode 配置 `limit` 缺少必填 `output` 字段导致 `ConfigInvalidError` 的问题——现在每个带 `context` 的模型条目自动补写 `output: 8192`
- 修正各工具配置写入格式，确保与客户端实际读取的字段一致：
  - **Gemini CLI**：改写 `~/.gemini/.env`（`GEMINI_API_KEY`/`GOOGLE_GEMINI_BASE_URL`/`GEMINI_MODEL`）而非 `settings.json` 的 `env` 对象，并在 `settings.json` 中设置 `security.auth.selectedType = "gemini-api-key"` 使 CLI 优先使用注入的 API Key
  - **Grok**：改用官方 `[models]` + `[model."<name>"]` 表结构（`model`/`base_url`/`name`/`api_key`/`api_backend`/`context_window`），不再使用 Codex 风格的 `model_providers` 布局
  - **Hermes**：`custom_providers` 改为序列格式（`- name:` 列表项），补充顶层 `model.provider` 字段
  - **OpenClaw**：provider 字段改用 camelCase（`baseUrl`/`apiKey`），新建配置自动声明 `models.mode = "merge"` 使 provider 累加
- 修复上游服务商表格点击小眼睛显示 API Key 后列变形的问题——API Key 列改用 flex 布局，span 设 `min-width:0` 允许收缩
- 预防性加固故障转移事件表和请求日志表的列宽约束，避免长文本撑宽表格

## [0.4.1] - 2026-08-08

### 修复
- 修复上游返回 429（限流）时不触发故障转移的问题——429 是上游自身状态，A 上游限流时 B 上游可能仍可用，现在会继续尝试下一个上游；若所有上游均限流，则透传真实 429 给调用方（便于退避重试）

## [0.4.0] - 2026-08-08

> ⚠️ 工具配置中部分需要待验证，我们在加紧完善，如有问题请加入 QQ 群反馈。

### 新增
- 新增 Anthropic 原生客户端端点 `POST /v1/messages` 与 Gemini 原生端点 `POST /v1beta/models/{model}:generateContent`，Claude Code / Gemini CLI 等原生工具可直接接入（支持非流式与流式双向转换，认证兼容 `x-api-key` / `x-goog-api-key`）
- 号池详情新增逐上游思考强度覆盖：可为池内每个上游单独设置思考强度（跟随池级/关闭/低/中/高/最大/自定义），覆盖池级配置
- 工具配置页新增系统环境变量冲突检测：当检测到 `ANTHROPIC_*`、`OPENAI_*`、`CLAUDE_CODE_*`、`GROK_*` 等系统环境变量会覆盖注入的代理配置时，页面给出警告并支持一键清理（清理前自动备份，可恢复）
- 工具配置新增异常退出恢复机制：若上次因进程被强制结束（kill）未能恢复原始配置，下次启动时自动从二级备份恢复
- 工具配置改用独立的工具网关 Token（自动生成并持久化），不再把默认 Gateway API Key 写入工具配置文件，主密钥更加安全
- Claude Desktop 支持按池真实上下文窗口声明 1M 能力：`pool-{name}` 路由与模型列表会依据池的推断窗口自动标记 `supports1m`
- Claude Desktop 写入前校验模型名合法性，形似 `claude-*` 但非法定模型的池名自动改写为代理路由，避免客户端拒收

### 变更
- 工具配置写入改用 Windows `ReplaceFileW` 原子替换（原为删除后重命名），消除非原子窗口；Claude Desktop / Hermes 按 PLAN 全量写入所有模型池
- 设置页日志保留默认值提高为 30 天 / 20000 条（与独立 Token 统计一致）
- 使用教程页新增「工具配置」「多格式 API 转换」章节，并同步更新上游服务、模型池、接入客户端示例章节
- Grok 配置补写认证令牌（`experimental_bearer_token`），清理系统 `XAI_*`/`GROK_*` 环境变量后仍可正常认证
- Gemini 配置补写默认模型（`GEMINI_MODEL`），并修正接口地址变量名为 CLI 实际识别的 `GOOGLE_GEMINI_BASE_URL`
- OpenClaw 配置改为全量写入所有模型池（含各池上下文窗口）与 `openai-completions` 协议声明，默认模型结构对齐客户端格式
- Hermes 配置改为按区块合并而非简单追加，用户已有的 `custom_providers`/`model` 配置不会再产生重复键
- Claude Code 配置接管认证时自动移除残留的 `ANTHROPIC_API_KEY`，避免与 `ANTHROPIC_AUTH_TOKEN` 并存触发客户端警告

## [0.3.4] - 2026-08-07

### 修复
- 修复长思考模型（如 DeepSeek R1、Claude thinking）被请求超时强制打断的问题——透传模式下不再设置请求级超时，仅保留建连超时与流式空闲超时兜底
- 修复上游返回 4xx 错误（如 400、429）时客户端只能看到统一 502 的问题——现在上游的 4xx 状态码和错误信息会原样透传给调用方，便于退避限流或修正请求参数

### 变更
- 优化设置页「信任 X-Forwarded-For」提示文案，新增帮助图标，点击可查看直连/反向代理模式下的详细说明

## [0.3.3] - 2026-08-06

### 修复
- 修复号池详情中拖拽行调整上游顺序功能失效的问题——Tauri v2 默认拦截 HTML5 拖拽事件，现已禁用原生拖拽拦截
- 修复请求日志缺少 Token 用量列的问题——日志表格新增 Token 列，展示每次请求的 Token 消耗
- 修复上游服务 API Key 小眼睛点开后列表变形的问题——限制 Key 列宽并添加省略号，鼠标悬停可查看完整 Key
- 修复所有复制操作无法进入剪切板的问题——新增系统级剪切板写入命令，统一所有复制入口使用三级回退策略

## [0.3.2] - 2026-08-06

### 修复
- 修复数据库时区不一致问题——日志写入和统计查询统一使用 `localtime`，解决仪表盘"今日请求"等数据为空的问题
- 修复小时统计 `strftime` 双重时区转换导致小时数偏移的问题
- 修复拖拽排序在 Windows WebView2 下失效的问题——补全 `dragenter` 事件监听
- 修复 API Key 无法查看的问题——新增"小眼睛"切换功能，支持在表格和编辑框中查看/隐藏明文 Key
- 修复轮询负载均衡日志缺失问题——注入 `tracing` 审计日志，追踪轮询计数器和上游选择过程
- 修复编辑模态框打开时 API Key 输入框可见性状态未重置的问题
- 修复上游 API Key 缓存未在列表刷新时清空的问题

## [0.3.1] - 2026-08-05

### 修复
- 紧急修复 v0.3.0 中 CSP 策略导致界面和功能完全不可用的问题——CSP 从 `null` 改为具体策略后阻断了 Tauri IPC 通信，所有后端命令调用失败，现已回滚为 `null`

## [0.3.0] - 2026-08-05

### 新增
- 智能 URL 规范化——上游 Base URL 支持多种输入格式：根地址、带 `/v1`、完整端点 URL、Google 非标准版本前缀（`/v1beta/openai`）等均可自动识别，不再产生路径重复拼接
- 测试连接改用真实对话请求——点击"测试连接"时发送 `max_tokens=1` 的极简 chat 请求，验证端到端通路（网络、认证、模型可用性、响应格式），不再仅依赖 `/models` 端点
- 健康检查和后台探测同步升级——有模型配置时使用真实 chat 请求探测，无模型时自动回退 `/models`

### 修复
- 修复上游健康状态更新的 TOCTOU 竞态条件——成功路径改用原子 SQL（`CASE WHEN`）消除"先读后写"竞态
- 修复 3 个数据库读操作误用写连接——`upstream_exists`、`count_upstreams`、`count_active_upstreams` 改用只读连接
- ~~配置内容安全策略（CSP）——替代原先的 `null` 策略，防止 XSS 注入~~（v0.3.1 已回滚，CSP 导致 Tauri IPC 通信中断）

### 优化
- 清理死代码——移除未使用的 `PoolRouter`、`RoundRobinRouter`、`ProxyEngine` 等模块
- 新增后台日志清理定时任务，自动清理过期日志
- 删除误导性的 `RateLimiter::update_config` 空实现

### 测试
- 新增 5 个集成测试（故障转移、认证失败、轮询分布、健康状态更新、模型名替换）
- 新增 28 个 URL 规范化单元测试
- 新增 3 个 `ChatTestResult` 序列化测试

## [0.2.2] - 2026-07-28

### 修复
- 修复日志页面折叠区块箭头方向与状态不一致的问题

## [0.2.1] - 2026-07-28

### 优化
- 简化日志页面布局——将统计信息改为可折叠区块，减少视觉混乱
- 移除冗余的请求统计区块——与请求记录列表信息重复，避免数据冗余展示
- 优化故障链展示——使用箭头连接符和紧凑标签，提升可读性

## [0.2.0] - 2026-07-28

### 新增
- 数据库读写分离——新增独立的只读连接（WAL 模式 + query_only），所有 SELECT 查询使用读连接，INSERT/UPDATE/DELETE 使用写连接，读写互不阻塞
- 限流状态持久化——限流计数状态持久化到 SQLite，应用重启后限流计数不丢失，过期条目自动清理
- 限流高并发优化——使用 DashMap 替代 Mutex<HashMap>，提供分片级别并发访问，避免全局锁争用
- 配置变更即时验证——上游编辑模态框新增"测试连接"按钮，即时验证 API Key 和 base_url 连通性并反馈结果（成功显示模型数+延迟，失败显示具体原因）
- 上游列表新增"测试"按钮，可快速验证已有上游的连通性
- 号池创建校验——新建号池时验证至少选择了一个上游
- 上游异常详情展示——上游列表点击异常行可展开查看错误原因、失败次数、最近失败/成功时间、恢复时间，错误自动分类（连接失败、认证失败、超时、服务器错误等）
- 错误趋势可视化——上游列表状态列显示失败次数指示点，直观反映错误频次
- 号池详情页——可视化展示池内上游关系，含排序编号、供应商、模型、健康状态
- 号池上游拖拽排序——在号池详情页拖拽行可调整上游顺序，自动保存
- 一键诊断包——导出包含版本信息、配置摘要、上游状态、最近日志和健康检查结果的 ZIP 压缩包，所有敏感信息（API Key 等）已脱敏处理
- 数据库备份与恢复——支持一键备份数据库为 .db 文件，从备份文件恢复（需重启生效），版本兼容性校验
- 自动备份策略——可配置自动备份间隔和最大备份数量，后台自动创建并清理旧备份
- 配置导入导出——支持导出所有上游、号池、设置为 JSON 文件，支持增量导入和全量导入两种模式
- 导入配置合法性校验——上游名称、base_url 格式、号池名称等字段导入前自动验证
- 多密钥访问控制——支持创建多个 Gateway API Key，可配置允许访问的号池和过期时间
- 阈值告警监控——后台监控请求失败率，超过阈值时记录告警并支持静默期防疲劳

### 修复
- 修复上游健康追踪在生产环境中完全失效的问题——`update_upstream_health` 失败路径误用只读连接（`query_only=1`）执行 UPDATE，导致健康状态永不更新
- 修复上游失败计数的 TOCTOU 竞态条件——改为原子 SQL `failure_count = failure_count + 1`，消除并发下的计数丢失
- 修复 SSE 流式响应中同一 JSON chunk 被解析两次的性能问题——合并为单次解析同时完成模型名替换、用量提取和错误检测
- 修复轮询计数器 Mutex 锁中毒时直接 panic 导致网关崩溃的问题——改为自动恢复并记录告警
- 修复思考模式参数合并中使用 `unwrap()` 可能 panic 的问题——改为安全解包
- 修复 HTTP 客户端构建失败时直接 panic 的问题——改为降级使用默认配置并记录错误
- 修复独立运行模式无法优雅关闭的问题——添加 Ctrl+C 信号处理，关闭时正确释放端口
- 修复 API Key 过期判断缺少格式校验的问题——添加 `YYYY-MM-DD HH:MM:SS` 格式验证，无效格式时 fail-open
- 修复告警监控任务在 Tauri setup 回调中调用 `tokio::spawn` 可能 panic 的问题——改为自建 tokio runtime
- 修复常量时间比较在密钥长度不同时提前返回导致的时序侧信道——改为按最大长度遍历
- 修复统计查询未使用只读连接的问题——`get_stats()` 改为使用读连接，遵循读写分离

## [0.1.22] - 2026-07-28

### 新增
- 探测配置 UI 支持——前端设置页可直接开关探测、调整间隔和失败阈值
- 数据库迁移幂等性——新增 `PRAGMA table_info` 列存在检测，异常中断后可安全重试
- 迁移回归测试——新增 v1→最新版完整迁移测试和 v6 部分修复测试，防止结构不一致回归

### 优化
- 统一探测配置加载逻辑——删除 `probe/mod.rs` 中重复代码，统一使用 `config::ProbeSettings`
- 集成测试覆盖——新增 18 个集成测试覆盖认证、路由、故障转移、SSE 流式代理等核心链路

### 修复
- 修复数据库迁移 v6 在特定情况下的失效问题——`ALTER TABLE` 改为逐条执行并配合列存在检测

## [0.1.21] - 2026-07-27

### 新增
- 添加请求速率限制功能——每 IP 每分钟最多 60 个请求，防止恶意刷接口
- 支持 `X-Forwarded-For` 头识别真实客户端 IP（反向代理场景）

### 优化
- 完善日志错误处理——数据库写入失败时记录 warning 日志，不再静默忽略
- 优化 SQLite 性能——启用 `synchronous=NORMAL`、`cache_size=-64000`、`temp_store=MEMORY`
- 补充边界测试——新增 null message 场景测试，修复 model_filter 中的 unwrap

### 修复
- 修复 stream response 构建时的 unwrap，改为安全错误处理

## [0.1.20] - 2026-07-27

### 修复
- 修复创建桌面快捷方式时弹出终端窗口的问题——添加 CREATE_NO_WINDOW 标志隐藏 PowerShell 窗口

## [0.1.19] - 2026-07-27

### 新增
- 更新完成后自动重命名 exe 文件为对应版本号（如 `LLM-API-Proxy_v0.1.19_x64_portable.exe`）
- 更新后自动更新桌面快捷方式指向新文件

## [0.1.18] - 2026-07-27

### 修复
- 修复下载完成后无法自动更新的问题——优化 PowerShell 更新脚本执行逻辑

## [0.1.17] - 2026-07-27

### 修复
- 修复下载完成后无法自动更新的问题——移除 DETACHED_PROCESS 标志并添加 500ms 延迟确保更新脚本正确启动

## [0.1.16] - 2026-07-27

### 修复
- 修复下载更新进度条无动画的问题——CSS 变量 `--primary` 未定义导致进度条背景透明

## [0.1.15] - 2026-07-27

### 修复
- 修复已知问题，提升稳定性

## [0.1.14] - 2026-07-27

### 修复
- 修复中文路径下更新失败的问题（使用 PowerShell EncodedCommand 避免编码问题）

## [0.1.13] - 2026-07-27

### 新增
- 首次启动时询问是否在桌面创建快捷方式
- 桌面快捷方式缺失时主动询问是否创建

## [0.1.12] - 2026-07-27

### 新增
- 点击「立即更新」后显示旋转进度动画，避免用户焦虑

### 修复
- 隐藏更新时的批处理小黑窗（使用 CREATE_NO_WINDOW 标志）

## [0.1.11] - 2026-07-27

### 修复
- 修复更新时批处理脚本报"找不到批处理文件"错误——改用绝对路径并添加 cd /d 确保工作目录正确

## [0.1.10] - 2026-07-27

### 修复
- 修复下载按钮点击无反应的问题——HTML onclick 属性中 JSON.stringify 双引号与属性定界符冲突导致解析失败

## [0.1.9] - 2026-07-27

### 修复
- 修复 GitHub 下载按钮不显示的问题——当 API 未返回下载链接时，使用版本号构造回退下载 URL

## [0.1.8] - 2026-07-27

### 修复
- 修复下载进度条不显示的问题——修复事件监听器竞态条件，下载前初始化进度事件和 100% 完成事件

## [0.1.7] - 2026-07-27

### 修复
- 修复日志时间、仪表盘统计、Token 消耗图表等所有时间相关功能使用 UTC 而非本地时区的问题

## [0.1.6] - 2026-07-27

### 新增
- 下载进度实时显示：流式下载通过 Tauri 事件推送进度条（百分比 + MB）
- 下载完成后弹出确认对话框：选择「立即更新」或「下次启动时更新」
- 标题栏新增语言切换按钮（中 / EN），无需进入设置即可切换界面语言
- 侧边栏检查更新按钮有新版本时变为绿色下载箭头，点击跳转设置页底部
- 启动时自动检测上次「稍后更新」的下载文件并自动应用

### 变更
- 更新流程拆分为两个阶段：download_update（仅下载）+ apply_update（退出并执行替换）
- 下载文件使用 _update_downloading.exe（下载中）和 _update_pending.exe（待安装）两阶段命名
- 下载中断时自动清理临时文件，避免残留

## [0.1.5] - 2026-07-27

### 修复
- 修复在线更新后桌面快捷方式图标消失的问题（更新完成后自动刷新 Windows 图标缓存）
- 修复更新时 exe 文件仍被占用导致替换失败的问题（增加重试等待机制）

### 变更
- 打包发布流程增加清理旧版本便携版文件的步骤

## [0.1.4] - 2026-07-27

### 新增
- 检查更新支持 GitHub + Gitee 双数据源，GitHub 访问失败时自动回退 Gitee
- 更新页面显示两个下载按钮（GitHub 下载 + Gitee 下载），方便国内用户选择

## [0.1.3] - 2026-07-27

### 变更
- 打包配置改为仅便捷版（便携版），移除 NSIS 安装包生成
- 完善开发规范：明确每次推送到 main 分支必须递增版本号

### 修复
- 修复启动检查发现新版本后侧边栏不显示更新提示的问题
- 修复设置页需手动点击检查才能显示更新信息的问题
- 修复下载按钮仅跳转网页而非直接下载更新的问题

## [0.1.2] - 2026-07-27

### 新增
- 支持中英文双语切换，可在设置页选择界面语言（中文 / English）
- 系统托盘菜单根据语言设置显示对应文本

### 修复
- 修复上游服务表格和模型池表格按钮文本因模板字符串语法错误导致显示异常的问题
- 修复主题切换按钮文本未跟随语言切换的问题

### 变更
- 轮播图、Token 消耗图表、上下文菜单、CSV 导出等所有动态内容均已支持国际化

## [0.1.1] - 2026-07-26

### 新增
- 设置页新增「版本更新」区域，支持检查 GitHub 最新发布版本
- 启动时自动静默检查更新，有新版本时弹出提示
- 侧边栏底部新增检查更新按钮，点击后显示更新状态
- 使用教程新增「版本更新」章节，说明便捷版手动下载更新流程
- 仪表盘「最近活动」区域改为展示真实请求日志，替代硬编码占位文本

### 修复
- 修复侧边栏检查更新按钮点击无反应的问题（ID 冲突导致 DOM 操作失败）

### 变更
- AGENTS.md 完善分支策略与版本发布规范（7.3 ~ 7.5 节）
- 侧边栏轮播图内容由广告替换为项目宣传语
- 移除右上角冗余的版本号显示

## [0.1.0] - 2026-07-25

### 新增
- **核心网关**：OpenAI 兼容 API 入口，支持 `/v1/chat/completions` 和 `/v1/models` 端点
- **多厂商聚合**：支持 DeepSeek、OpenAI、智谱 GLM、Grok、Claude 等厂商 API 统一接入
- **轮询负载均衡**：两层轮询机制（上游轮询 + 模型轮询），请求均匀分布
- **自动故障转移**：上游失败时自动切换下一个上游，支持 HTTP 5xx 和响应体 error 检测
- **SSE 流式透传**：完整支持流式响应，实时替换 model 字段，异步提取 Token 用量
- **思考模式注入**：按厂商类型自动注入推理参数（DeepSeek reasoning、OpenAI reasoning_effort、Claude thinking）
- **加密存储**：API Key 使用 AES-256-GCM 加密，明文不落盘
- **桌面 GUI**：Tauri v2 原生窗口，可视化配置上游和模型池，无需命令行
- **请求日志**：记录所有转发请求的状态码、耗时、Token 用量、失败上游链
- **健康检查**：一键检测所有上游连通性和响应延迟
- **双击即用**：单文件 `.exe`，无需 Docker / Node.js / Python 环境
- **数据便携**：所有数据存储在程序同目录 `data/` 文件夹，U 盘携带即走
- **开源治理**：AGPL-3.0 双轨许可、CLA 贡献者协议、CONTRIBUTING.md 贡献指南

[Unreleased]: https://github.com/jiz-1789/LLM-API-Proxy/compare/v0.4.2...HEAD
[0.4.2]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.4.2
[0.4.1]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.4.1
[0.4.0]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.4.0
[0.3.4]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.3.4
[0.3.3]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.3.3
[0.3.2]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.3.2
[0.3.1]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.3.1
[0.3.0]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.3.0
[0.2.2]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.2.2
[0.2.1]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.2.1
[0.2.0]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.2.0
[0.1.22]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.22
[0.1.21]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.21
[0.1.20]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.20
[0.1.19]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.19
[0.1.18]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.18
[0.1.17]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.17
[0.1.16]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.16
[0.1.15]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.15
[0.1.7]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.7
[0.1.6]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.6
[0.1.5]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.5
[0.1.4]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.4
[0.1.3]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.3
[0.1.2]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.2
[0.1.1]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.1
[0.1.0]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.0
