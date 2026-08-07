# AGENTS.md — AI 协作规则

> 本文件是所有 AI 编码助手在本仓库中工作时的**强制规范**。每条规则均来源于项目实际代码模式和设计文档，违反规则可能导致数据丢失、安全漏洞或编译失败。

---

## 一、项目概览

LLM-API-Proxy 是一个本地运行的大模型 API 代理网关，将多家 LLM 厂商的 API 聚合为一个 OpenAI 兼容入口，提供轮询负载均衡、故障转移、SSE 流式透传和加密存储。

**技术栈**：Rust + Tauri v2 + SQLite + Axum

**架构分层**：

```
dist/index.html              ← 前端单页应用（内嵌 CSS/JS，Tauri Webview 加载）
src-tauri/src/commands/      ← Tauri 命令层（模块化，GUI ↔ 后端桥接）
src/lib.rs                   ← 后端初始化 + AppState
src/gateway/                 ← OpenAI 兼容网关（Axum 路由 + 认证 + 限流 + SSE）
src/pool/                    ← 号池与思考模式注入
src/proxy/                   ← 上游转发与故障转移
src/db/                      ← SQLite 数据层（模块化：迁移 + CRUD + 事务 + 读写分离）
src/crypto.rs                ← AES-256-GCM 加解密
src/config.rs                ← 配置与路径管理
src/error.rs                 ← 统一错误类型
src/probe/                   ← 后台上游探测
src/alert/                   ← 阈值告警监控
src/diagnostic.rs            ← 一键诊断包导出
src/config_io.rs             ← 配置导入导出
```

**数据流**：客户端请求 → Axum 网关 → 认证 → 查找号池 → 轮询选择上游 → 注入思考参数 → 转发至上游 → 模型名替换 → 记录日志 → 返回客户端

---

## 二、数据库变更规范（最高优先级）

> **核心原则：所有数据库变更必须兼容旧版本，禁止任何会导致用户更新后数据丢失的操作。**
> 详见 `DEVELOPMENT.md`。

### 2.1 迁移规则

| 规则 | 说明 |
|------|------|
| ✅ 只允许 `ALTER TABLE ADD COLUMN` | 新增列，不影响旧数据 |
| ✅ 新增列必须有 `DEFAULT` 值（可空列除外） | 旧行自动填充默认值 |
| ✅ 新建表使用 `CREATE TABLE IF NOT EXISTS` | 幂等创建 |
| ✅ 新建索引使用 `CREATE INDEX IF NOT EXISTS` | 幂等创建 |
| ❌ 禁止 `DROP COLUMN` | SQLite 支持有限且会丢数据 |
| ❌ 禁止 `DROP TABLE` 再重建 | 直接丢失所有数据 |
| ❌ 禁止修改列名或列类型 | 旧数据不兼容 |

### 2.2 添加新迁移的步骤

1. 在 `src/db/migration.rs` 的 `run_migrations()` 方法中，向迁移逻辑末尾追加新版本
2. 版本号**严格递增 +1**，不可跳号
3. 当前最新版本为 **v14**（参见 `DEVELOPMENT.md` 迁移历史表）
4. 同步更新 `DEVELOPMENT.md` 的 Schema 表和迁移历史表

```rust
// ✅ 正确示例
(6, "ALTER TABLE request_logs ADD COLUMN latency_category TEXT;"),
```

### 2.3 代码层兼容性

新增列后，必须同步更新以下代码：

- `map_*_row` 函数中的列索引（`row.get(N)`）
- `INSERT` 语句包含新列
- 读取时处理 `NULL` / 空值（使用 `unwrap_or` 或 SQL `COALESCE`）

```rust
// ✅ 读取可空列的正确方式
let model: Option<String> = row.get(4)?;
let model = model.unwrap_or_else(|| "未记录".to_string());
```

### 2.4 需要修改列语义时

采用**新增列 + 数据回填**策略，绝不删除旧列：

```rust
// 1. 新增列
(6, "ALTER TABLE upstreams ADD COLUMN api_key_v2 BLOB;"),
// 2. 代码层双读兼容：优先读 v2，为空则读 v1
```

### 2.5 事务安全

- 写操作使用 `with_transaction()` 方法，事务期间持有 Mutex 锁防止并发干扰
- 闭包返回 `Err` 时自动回滚
- 不要在事务闭包内执行耗时操作（如网络请求）

---

## 三、代码风格与约定

### 3.1 Rust 代码

- **Edition**：workspace 使用 `2024`，src-tauri 使用 `2021`
- **错误类型**：所有业务错误使用 `AppError`（定义于 `src/error.rs`），通过 `#[from]` 自动转换
- **错误传播**：使用 `?` 运算符，不要手动 `unwrap()` 可恢复的错误
- **日志**：使用 `tracing` 宏（`info!`/`warn!`/`debug!`/`error!`），不要用 `println!`
- **注释**：公开 API 使用 `///` 文档注释；函数参数上的注释使用普通 `//`（`///` 在参数上会导致编译错误）
- **命名**：模块和文件用 `snake_case`，类型用 `PascalCase`，常量用 `SCREAMING_SNAKE_CASE`
- **派生**：数据结构统一派生 `Debug, Clone, Serialize, Deserialize`

```rust
// ✅ 标准数据结构定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Upstream {
    pub id: String,
    pub provider_name: String,
    // ...
}
```

### 3.2 Tauri 命令层（`src-tauri/src/commands/`）

- 每个命令标注 `#[tauri::command]`
- 命令函数返回 `Result<T, String>`（错误转为字符串传给前端）
- 同步命令用 `pub fn`，涉及网络请求的用 `pub async fn`
- 使用 `State<'_, AppState>` 获取共享状态
- 每新增命令必须在 `src-tauri/src/lib.rs` 的 `invoke_handler!` 宏中注册

```rust
#[tauri::command]
pub fn list_upstreams(state: State<'_, AppState>) -> Result<Vec<UpstreamVO>, String> {
    state.db.get_upstreams()
        .map(|ups| ups.iter().map(to_vo).collect())
        .map_err(|e| e.to_string())
}
```

### 3.3 网关层（`src/gateway/`）

- 路由注册在 `create_router()` 函数中
- OpenAI 兼容端点路径以 `/v1/` 开头
- 管理端点路径以 `/api/` 开头
- API Key 认证使用 `auth::validate_api_key()`，内部用常量时间比较防侧信道
- 请求日志使用 `uuid::Uuid::new_v4().simple()` 生成唯一 `log_id`，避免碰撞

### 3.4 前端（`dist/index.html`）

- 单文件 SPA，CSS 和 JS 内嵌于 HTML
- 前端通过 `window.__TAURI__.core.invoke('command_name', { args })` 调用后端命令
- 动态拼接 HTML 时，必须使用 `jsEsc()` 函数对用户输入进行 XSS 转义
- 支持 `data-theme="light"` / `data-theme="dark"` 主题切换
- 所有用户可见文本使用中文

---

## 四、安全规范

### 4.1 API Key 处理

- API Key **只在内存中解密使用**，明文绝不落盘
- 加密使用 AES-256-GCM，Master Key 存储在 `data/master_key.bin`
- Master Key 生成使用 `getrandom`（密码学安全随机数）
- Master Key 写入使用临时文件 + rename 原子操作
- 前端展示时 API Key 脱敏为 `••••••••`

### 4.2 认证与授权

- Gateway API Key 校验使用**常量时间比较**（`constant_time_eq`），防止时序侧信道攻击
- 外部链接打开前校验 URL 协议白名单（仅允许 `http`/`https`），防止命令注入
- 默认仅监听 `127.0.0.1`，不对外网开放

### 4.3 前端安全

- 动态内容输出经过 HTML 转义（`jsEsc` 函数）
- `onclick` 等内联事件属性中的参数必须转义
- CSP 由 Tauri 管理，不随意放宽

### 4.4 并发安全

- SQLite 连接使用 `Mutex<Connection>` 保护
- 事务期间持有 Mutex 锁，防止并发语句交错
- 轮询计数器使用 `Arc<Mutex<HashMap>>` 保护
- Master Key 在 `Arc` 中共享，只读访问无需加锁

---

## 五、网关与代理逻辑规范

### 5.1 请求生命周期

```
1. 收到 /v1/chat/completions 请求
2. 验证 Gateway API Key
3. 解析 model 字段 → 查找匹配的号池
4. 获取号池的上游列表（按 sort_order 排序）
5. 轮询选择起始上游
6. 对每个上游：
   a. 检查是否启用（跳过禁用的）
   b. 解密 API Key
   c. 构建请求体（覆盖 model、注入 thinking 参数）
   d. 流式 → forward_stream_request / 非流式 → forward_request
   e. 成功 → 替换 model 为 display_name、记录日志、返回
   f. 失败 → 记录失败信息、继续下一个上游
7. 所有上游失败 → 返回 502（有尝试过）或 503（全部禁用）
```

### 5.2 故障转移判定

以下情况触发故障转移（跳到下一个上游）：

- HTTP 连接错误或超时
- 上游返回 HTTP 5xx
- 上游返回 HTTP 200 但响应体包含 `error` 字段（"假成功"检测）
- 流式响应中 SSE chunk 包含 `error` 字段

### 5.3 模型名替换

- 发给上游的 `model` 字段使用上游配置的实际模型名（或号池中为该上游指定的模型）
- 返回给客户端的 `model` 字段替换为号池的 `display_name`
- 流式响应中每个 SSE chunk 的 `model` 字段也要替换

### 5.4 思考模式注入

- 按上游的 `provider_name` 判断厂商类型，**逐个上游**注入对应参数
- 客户端可通过 `"reasoning": false` 强制关闭思考模式
- 厂商映射表（`src/pool/thinking.rs`）：

| 厂商关键词 | 注入参数 |
|------------|----------|
| `deepseek` / `ds` | `{"reasoning": true}` |
| `openai` / `gpt` | `{"reasoning_effort": "high"}` |
| `claude` / `anthropic` | `{"thinking": {"type": "enabled"}}` |
| 其他 | 不注入 |

### 5.5 日志记录

- 每次请求记录一条 `request_logs`，包含：状态码、耗时、是否流式、Token 用量、失败上游链
- `log_id` 和 `request_id` 使用 UUID v4 生成，防止碰撞
- 流式请求在结束后异步更新 Token 用量
- 日志筛选支持 `status_prefix`（如 `5` 匹配所有 5xx），使用 SQL 范围查询而非精确匹配

---

## 六、测试规范

### 6.1 单元测试

- 测试代码与源码同文件，放在 `#[cfg(test)] mod tests` 块中
- 使用标准 `#[test]` 属性
- 加密模块测试使用 `tempfile::TempDir` 隔离文件系统
- 轮询和思考模式注入必须有测试覆盖

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let dir = TempDir::new().unwrap();
        let km = KeyManager::initialize(dir.path()).unwrap();
        let plaintext = "sk-test-api-key-12345";
        let encrypted = km.encrypt_api_key(plaintext).unwrap();
        let decrypted = km.decrypt_api_key(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
```

### 6.2 验证清单

每次提交前自检：

- [ ] `cargo build` 编译通过
- [ ] `cargo clippy` 无 WARNING / ERROR
- [ ] `cargo test` 全部通过
- [ ] 涉及数据库变更时，已在旧数据库上测试升级路径
- [ ] 新增 Tauri 命令已在 `invoke_handler!` 中注册
- [ ] 前端动态内容已做 XSS 转义
- [ ] API Key 明文不出现在日志中

### 6.3 新增功能必须补最小验证测试

> **核心约束：所有新增功能必须附带最小验证测试，无测试的 PR 不予合并。**

#### 约束范围

| 新增内容 | 最小测试要求 |
|----------|-------------|
| 新增 Tauri 命令 | 至少补充输入校验和错误传播测试 |
| 新增网关端点 | 至少补充认证和基本路由测试 |
| 新增数据库操作 | 至少补充 CRUD 回归测试 |
| 新增配置项 | 至少补充默认值和异常值加载测试 |
| 新增认证/安全逻辑 | 必须补充边界条件和权限校验测试 |

#### 测试命名规范

- 测试函数以 `test_` 开头，描述被测行为而非实现细节
- 使用有意义的断言消息，便于定位失败原因
- 测试代码与源码同文件，放在 `#[cfg(test)] mod tests` 块中

#### PR 审查检查点

审查 PR 时，检查以下内容：

- [ ] 新增功能是否附带对应的测试
- [ ] 测试是否覆盖了正常路径和错误路径
- [ ] 测试是否覆盖了边界条件（空值、极值、异常输入）
- [ ] 测试在 `cargo test` 中全部通过

---

## 七、Git 提交规范

### 7.1 Commit Message 格式

采用 Conventional Commits，描述使用中文：

```
<type>(<scope>): <描述>
```

| type | 用途 |
|------|------|
| `feat` | 新功能 |
| `fix` | Bug 修复 |
| `perf` | 性能优化 |
| `refactor` | 重构（不改行为） |
| `test` | 测试相关 |
| `docs` | 文档更新 |
| `build` | 构建/打包配置 |
| `style` | 样式/UI 调整 |
| `chore` | 杂项（gitignore 等） |

**实际示例**（参考项目 git log）：

```
feat: 多模型负载均衡、单实例运行、UI优化与使用教程更新
fix: 修复模型标签选中状态不更新+柱状图高度过高
perf: single-pass SSE chunk processing to avoid double JSON parse
docs: 新增开发规范与数据库兼容性文档
build: 打包配置改为仅便捷版（便携版），移除 NSIS 安装包
chore: gitignore internal docs and test data
```

### 7.2 提交粒度

- **一任务一 commit**：每个功能点或修复独立提交
- **原子性**：一个 commit 只做一件事
- **可回滚**：每个 commit 可独立 revert

### 7.3 分支策略

> **核心原则：main 是唯一的稳定发布分支，只提供最终稳定版本。任何开发工作都必须在分支上进行，严禁直接在 main 上修改代码。**

- `main`：**稳定发布分支**，只接受经过验证的合并，**严禁**直接在 `main` 上开发
- `feature/*`：功能开发分支，从 `main` 切出，开发完成后合并回 `main`
- `fix/*`：Bug 修复分支，从 `main` 切出，修复后合并回 `main`
- `hotfix/*`：紧急修复分支，从 `main` 切出，修复后合并回 `main`

**分支命名规范：**

| 类型 | 命名格式 | 示例 |
|------|----------|------|
| 新功能 | `feature/简短描述` | `feature/shortcut-dialog` |
| Bug 修复 | `fix/问题描述` | `fix/memory-leak` |
| 紧急修复 | `hotfix/问题描述` | `hotfix/crash-on-startup` |

**分支工作流：**

```
1. 从 main 切出 feature/xxx 分支
2. 在 feature 分支上开发和自测
3. 自测通过后，等待用户确认（代码审查 + 功能验证）
4. 用户确认通过后，合并到 main
5. 更新版本号、打包、上传代码到仓库
6. 清理已合并的 feature 分支
```

**规则：**

- ❌ **严禁在 `main` 分支上进行任何开发工作**（包括功能开发、Bug 修复、文档更新等）
- ✅ 所有功能开发必须在 `feature/*` 分支上进行
- ✅ 所有 Bug 修复必须在 `fix/*` 分支上进行
- ✅ 紧急修复可从 `main` 切出 `hotfix/*` 分支，修复后合并回 `main`
- ✅ 已合并的分支应及时清理，避免分支堆积
- ❌ **禁止未经用户确认直接将代码合并到 main** — 开发完成后必须等待用户审查和确认
- ❌ **禁止未经用户确认推送任何分支到远程仓库** — 开发期间（feature/fix/hotfix 分支）只在本地产出提交，**不执行 `git push`**；本地完成开发、自测全部通过后，将结果汇报给用户，由用户决定是否推送远程（推送哪些分支、何时推送，均以用户指令为准）
- ✅ 用户确认合并 main 时，按 7.4 流程执行（版本号、CHANGELOG、合并、打标签）；**合并 main 后的推送动作同样等待用户明确指令**，不自动推送

### 7.4 版本号管理

本项目采用 **语义化版本**（Semantic Versioning），格式为 `vMAJOR.MINOR.PATCH`：

| 版本段 | 何时递增 | 示例 |
|--------|----------|------|
| MAJOR | 不兼容的 API 变更 | `v1.0.0` |
| MINOR | 向下兼容的新功能 | `v0.2.0` |
| PATCH | 向下兼容的 Bug 修复 | `v0.1.1` |

**版本号位置（三处必须同步）：**

| 文件 | 字段 | 当前值 |
|------|------|--------|
| `Cargo.toml`（workspace） | `[workspace.package] version` | `0.2.0` |
| `src-tauri/Cargo.toml` | `[package] version` | `0.2.0` |
| `src-tauri/tauri.conf.json` | `"version"` | `0.2.0` |

> **⚠️ 每次推送到 `main` 分支必须递增版本号。** 无论是新功能、Bug 修复还是文档更新，合并到 main 前必须同步更新三处版本号和 `CHANGELOG.md`，确保每个 main 提交都对应一个唯一的发布版本。版本递增规则：Bug 修复 / 文档 → PATCH，新功能 → MINOR。

**发布流程（必须使用 `publish.ps1` 脚本）：**

```
1. 在 feature/fix 分支完成开发并自测通过
2. 更新三处版本号（递增 PATCH 或 MINOR）
3. 更新 CHANGELOG.md，添加版本说明
4. 合并到 main
5. 提交版本号变更：git commit -m "build: 发布 vX.Y.Z"
6. 打 Git 标签：git tag vX.Y.Z
7. 推送代码和标签：git push origin main && git push origin vX.Y.Z
8. 构建便携版：cargo tauri build（仅生成 release exe，不打包安装程序）
9. 使用发布脚本发布（禁止手动发布）：
   .\publish.ps1 X.Y.Z -GitHub -Gitee
```

**`publish.ps1` 脚本说明：**

项目根目录提供 `publish.ps1` 脚本，统一处理发布流程：

| 参数 | 说明 | 示例 |
|------|------|------|
| `Version` | **必填**，版本号（不带 `v` 前缀） | `0.2.0` |
| `-GitHub` | 发布到 GitHub Releases | `-GitHub` |
| `-Gitee` | 发布到 Gitee Releases | `-Gitee` |

**脚本功能：**
- 自动读取 `.env` 文件中的 Token（`GITHUB_TOKEN`、`GITEE_TOKEN`）
- 自动从 `CHANGELOG.md` 解析对应版本的发布说明
- 自动创建 Release 并上传便携版 exe
- GitHub 使用 REST API 上传，Gitee 使用 curl 上传（更可靠）

**使用示例：**

```powershell
# 同时发布到 GitHub 和 Gitee（推荐）
.\publish.ps1 0.2.0 -GitHub -Gitee

# 仅发布到 GitHub
.\publish.ps1 0.2.0 -GitHub

# 仅发布到 Gitee
.\publish.ps1 0.2.0 -Gitee
```

> **⚠️ 强制规定：必须使用 `publish.ps1` 脚本发布，禁止手动调用 API 或网页上传。** 脚本确保发布流程标准化，避免人为错误（如版本号不一致、更新日志遗漏、上传文件错误等）。

> **注意：发布需要 Personal Access Token（PAT）。** Token 存储在 `.env` 文件中（已加入 `.gitignore`，不会提交到仓库）。获取 Token 后，复制 `.env.example` 为 `.env` 并填入 Token。建议创建仅含 `repo` 权限的 Token，并在发布后定期轮换。

**打包规则：**

- ✅ **只打包便捷版（便携版）**：`tauri.conf.json` 中 `bundle.targets` 设为 `["app"]`，仅编译生成 release exe，不生成 NSIS 安装包
- ✅ 便携版 exe 路径：`target/release/llm-api-proxy-app.exe`
- ✅ 发布时重命名为 `LLM-API-Proxy_vX.Y.Z_x64_portable.exe`
- ❌ 禁止打包 NSIS 安装版（`bundle.targets` 不得包含 `nsis`）
- ❌ 禁止上传安装版到 GitHub Release Assets

**版本规则：**

- ❌ 禁止跳过版本号递增直接打包发布
- ❌ 禁止使用 `v0.1.0-r1`、`v0.1.0-beta` 等非标准后缀作为正式发布标签
- ❌ 禁止推送到 main 时不更新版本号（每次 main 推送必须递增版本号）
- ✅ Git 标签格式必须为 `vX.Y.Z`（如 `v0.1.1`、`v0.2.0`）
- ✅ 每次发布必须同步更新 `CHANGELOG.md`
- ✅ GitHub Release 的 `tag_name` 必须与三处版本号一致（带 `v` 前缀）

### 7.5 更新日志（CHANGELOG）

项目根目录维护 `CHANGELOG.md`，记录每个版本的变更内容：

- 格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)
- 每次发布新版本前，在文件顶部新增版本条目
- 变更分为：`新增`、`变更`、`修复`、`移除` 四类
- 从用户视角描述变更，不写文件名、重构细节等内部信息

**更新日志填写规范：**

| 更新类型 | 填写方式 | 示例 |
|----------|----------|------|
| 大更新（新功能、重大改进） | 完整填写所有变更内容，按类别分组 | `### 新增`、`### 修复` 等详细列出 |
| 小更新（Bug 修复、小优化） | 可简化为"修复已知问题" | `### 修复` 下列出主要问题或简化描述 |

```markdown
## [0.1.1] - 2026-07-26

### 新增
- 设置页新增版本检查功能，可检测 GitHub 最新发布版本

### 修复
- 修复仪表盘最近活动显示硬编码文本的问题
```

```markdown
## [0.1.2] - 2026-07-27

### 修复
- 修复已知问题，提升稳定性
```

---

## 八、禁止事项

| ❌ 禁止 | 原因 |
|---------|------|
| 在 `println!` / `eprintln!` 中输出 API Key 明文 | 安全风险 |
| 使用 `unwrap()` 处理可恢复错误 | 可能 panic 导致服务崩溃 |
| 在事务闭包内执行网络请求 | 长时间持有 Mutex 锁阻塞其他请求 |
| `DROP COLUMN` / `DROP TABLE` | 数据丢失 |
| 跳号迁移版本 | 升级路径断裂 |
| 直接拼接 SQL 字符串（非参数化查询） | SQL 注入风险 |
| 在 `onclick` 属性中直接插入未转义的用户输入 | XSS 漏洞 |
| 在函数参数上使用 `///` 文档注释 | 编译错误 |
| 修改 `master_key.bin` 格式或位置 | 旧用户数据无法解密 |
| 在 `src-tauri/gen/` 下手动编辑文件 | 自动生成，会被覆盖 |
| 提交 `target/`、`*.db`、`*.exe`、`master_key.bin` | 构建产物/敏感数据 |
| 在 `main` 分支上进行任何开发工作 | 绕过分支审查流程，main 仅提供稳定版本 |
| 不更新版本号直接打包发布 | 版本混乱，用户无法检测更新 |
| 推送到 main 时不递增版本号 | 每次 main 推送必须对应唯一版本 |
| 打包 NSIS 安装版 | 项目仅发布便捷版（便携版） |
| `bundle.targets` 包含 `nsis` | 仅允许 `["app"]`，生成裸 exe |
| 三处版本号不一致 | 语义化版本失效 |
| 发布时不更新 `CHANGELOG.md` | 用户无法了解变更内容 |

---

## 九、新增功能时的检查路径

添加新功能时，按以下路径检查需要同步修改的文件：

```
新数据库字段
  → src/db/migration.rs (migration)
→ src/db/<table>.rs (map_row + INSERT)
  → DEVELOPMENT.md (Schema 表 + 迁移历史)
  → src-tauri/src/commands/<module>.rs (DTO + 命令)
  → dist/index.html (前端表单 + 展示)

新 Tauri 命令
  → src-tauri/src/commands/<module>.rs (命令函数)
  → src-tauri/src/lib.rs (invoke_handler! 注册)

新网关端点
  → src/gateway/mod.rs (路由注册 + handler)
  → src/gateway/auth.rs (如需认证)

新上游厂商
  → src/pool/thinking.rs (thinking 参数映射)

版本发布
  → Cargo.toml (workspace version)
  → src-tauri/Cargo.toml (package version)
  → src-tauri/tauri.conf.json (version)
  → CHANGELOG.md (版本更新说明)
  → git tag vX.Y.Z
  → cargo tauri build (仅便携版 exe)
  → 重命名为 LLM-API-Proxy_vX.Y.Z_x64_portable.exe
  → GitHub Release 上传便携版
```

---

## 十、参考资料

| 文档 | 说明 |
|------|------|
| `README.md` | 项目介绍与使用指南 |
| `AGENTS.md` | AI 协作规则（本文件） |
| `CONTRIBUTING.md` | 贡献指南与提交流程 |
| `CLA.md` | 贡献者许可协议 |
| `LICENSE` | 双轨许可（AGPL-3.0 + 商业授权） |
| `NOTICE` | 版权声明摘要 |
| `CHANGELOG.md` | 版本更新日志 |
| `DEVELOPMENT.md` | 数据库 Schema、迁移规范与兼容性检查清单 |
| `LLM-API-Proxy-PRD.md` | 产品需求文档（内部，未公开） |
| `LLM-API-Proxy-TECHDESIGN.md` | 技术设计文档（内部，未公开） |
| `plan.md` | 开发计划与里程碑（内部，未公开） |

> PRD、技术设计、开发计划文档为内部文档，已通过 `.gitignore` 屏蔽，不会上传至 GitHub。
