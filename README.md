# LLM-API-Proxy

[English](./README_EN.md) | 简体中文

统一管理多厂商大模型 API 的本地代理网关，带原生桌面 GUI 窗口。

将多家 LLM 厂商（DeepSeek、OpenAI、智谱 GLM、Grok、Claude 等）的 API 聚合为**一个 OpenAI 兼容入口**，自动轮询负载均衡、故障转移，终端工具只需配置一次即可无缝切换上游。

## 🤔 为什么需要这个项目

如果你同时订阅了多家大模型 API（DeepSeek、OpenAI、智谱、Grok、Claude……），日常使用中一定遇到过这些麻烦：

- **每家厂商都要单独配置** — URL、API Key、模型名各不相同，在 ChatGPT、Claude Desktop、Cursor 等客户端里反复切换，头晕眼花
- **某家厂商突然挂了** — 手动换 Key、换地址，等修好了再换回来，全程人工盯着
- **想轮询分摊额度** — 同一个问题发到不同厂商，手动轮换根本不现实

市面上的聚合中转方案大多是这样：

| 痛点 | 典型方案 | 实际体验 |
|------|----------|----------|
| 部署复杂 | 需要 Docker / Node.js 环境，一堆 `docker-compose` 配置 | 小白看到命令行就劝退 |
| 配置繁琐 | 改 YAML / JSON 配置文件，手动维护上游列表 | 改错一个缩进整个服务起不来 |
| 无法携带 | 部署在服务器或固定机器上，换电脑要重配 | 公司配一套、家里配一套，永远对不上 |
| 需要运维 | 关注容器状态、日志、端口、防火墙 | 用个 API 代理还要学 DevOps？ |
| 数据安全 | API Key 明文存在配置文件里 | 谁拿到文件谁就能用你的额度 |

**LLM-API-Proxy 就是为了解决这些问题而生的：**

| 优势 | 本项目 | 对小白意味着什么 |
|------|--------|------------------|
| **双击即用** | 单文件 `.exe`，无需 Docker / Node.js / Python | 下载 → 双击 → 开始用，跟装 QQ 一样简单 |
| **可视化 GUI** | 原生桌面窗口，填表单代替改配置文件 | 所见即所得，不用记任何命令 |
| **U 盘便携** | 整个文件夹复制到 U 盘，插到任意电脑就能用 | 公司 ← → 家里 ← → 笔记本，配置随身带 |
| **一键获取模型** | 填好 URL 和 Key，点一下自动拉取可用模型列表 | 不用翻文档查模型名叫什么 |
| **自动故障转移** | 某家挂了自动跳下一个，全程无感知 | 再也不用半夜爬起来换 Key |
| **API Key 加密** | AES-256-GCM 加密存储，明文不落盘 | 不怕别人翻你的文件 |
| **在线更新** | 内置版本检查，一键下载更新（支持 GitHub / Gitee 双源） | 国内网络也能顺畅更新 |
| **中英双语** | 界面支持中文 / English 一键切换 | 不论母语都能轻松上手 |
| **零运维** | 关窗口最小化到托盘，后台默默跑着 | 当它不存在就好 |

> **一句话总结**：如果你不想折腾 Docker、不想背命令行、不想每换一台电脑就重配一遍 API，那这个工具就是为你做的。

## ✨ 核心特性

- **统一入口** — 对外暴露一个 OpenAI 兼容的 URL 和 API Key，兼容 ChatGPT、Claude Desktop、LibreChat、FastGPT 等主流客户端
- **号池聚合** — 将多个厂商的同名/同类模型聚合到一个自定义号池中，对外表现为一个"模型"
- **顺序轮询** — 每次请求按轮询顺序分配给不同上游，实现负载均衡
- **故障转移** — 某个上游失败时自动跳过并尝试下一个，支持 HTTP 错误、超时、以及响应体 `error` 字段检测
- **SSE 流式透传** — 完整透传上游 SSE 事件流，支持流式聊天与 Token 用量统计
- **思考模式** — 按上游厂商类型自动注入 `reasoning` / `reasoning_effort` / `thinking` 参数
- **模型伪装** — 终端工具看到的模型名始终是号池名，底层实际模型名被隐藏
- **AES-256-GCM 加密** — API Key 加密后存入 SQLite，Master Key 跟随数据目录，U 盘即插即用
- **请求日志** — 记录每次请求的状态码、耗时、Token 用量、失败上游链，支持按状态码/时间筛选与导出
- **健康检查** — 一键测试所有上游连通性
- **在线更新** — 启动时自动检查新版本，支持 GitHub + Gitee 双数据源，一键下载并自动替换重启
- **中英双语** — 界面支持中文 / English 切换，系统托盘菜单跟随语言设置
- **系统托盘** — 关闭窗口最小化到托盘，后台继续提供代理服务
- **明暗主题** — 支持亮色/暗色模式切换，持久化保存

## 🛠 技术栈

| 模块 | 技术 |
|------|------|
| 后端框架 | Axum 0.8 (Rust) |
| 桌面 GUI | Tauri v2 |
| 数据库 | SQLite (rusqlite, WAL 模式) |
| 加密 | AES-256-GCM (aes-gcm) |
| HTTP 客户端 | reqwest 0.12 |
| 异步运行时 | tokio |
| 日志 | tracing + tracing-subscriber |
| 前端 | 原生 HTML/CSS/JS（内嵌于 Tauri Webview） |

## 📁 项目结构

```
.
├── Cargo.toml              # Workspace 根配置
├── dev-server.js           # 开发模式静态文件服务器
├── package.json
├── src/                    # Rust 核心库（后端逻辑）
│   ├── lib.rs              # 模块声明 + AppState + 后端初始化
│   ├── config.rs           # Gateway 配置与路径管理
│   ├── crypto.rs           # AES-256-GCM 加解密 + Master Key 管理
│   ├── db.rs               # SQLite 数据层（CRUD + 迁移 + 事务）
│   ├── error.rs            # 统一错误类型
│   ├── gateway/            # OpenAI 兼容网关
│   │   ├── mod.rs          # /v1/models, /v1/chat/completions 路由
│   │   ├── auth.rs         # API Key 认证（常量时间比较防侧信道）
│   │   └── stream.rs       # SSE 流式处理（模型名替换 + Token 提取）
│   ├── pool/               # 号池与轮询逻辑
│   │   ├── mod.rs          # Pool 数据结构
│   │   ├── round_robin.rs  # 顺序轮询选择器
│   │   └── thinking.rs     # 思考模式参数注入（按厂商映射）
│   └── proxy/              # 上游转发
│       ├── mod.rs          # ProxyEngine
│       ├── client.rs       # UpstreamConfig 定义
│       ├── failover.rs     # HTTP 转发与故障转移链
│       └── model_filter.rs # 模型名替换
├── src-tauri/              # Tauri 桌面应用外壳
│   ├── tauri.conf.json     # 窗口/打包配置
│   ├── capabilities/       # Tauri 权限配置
│   ├── icons/              # 应用图标
│   └── src/
│       ├── lib.rs          # Tauri 应用入口 + 系统托盘 + 窗口事件
│       ├── main.rs         # main 入口
│       └── commands.rs     # Tauri 命令（GUI ↔ 后端桥接）
└── dist/                   # 前端构建产物
    └── index.html          # 单页应用（内嵌 CSS/JS）
```

## 🚀 快速开始

### 环境要求

- **Rust** 1.77+（推荐最新 stable）
- **Tauri CLI** 2.0+
- **Node.js** 18+（仅开发模式需要）
- **Windows** 10/11（当前仅支持 Windows）

### 开发模式

```bash
# 1. 安装 Tauri CLI
cargo install tauri-cli --version "^2"

# 2. 启动开发服务器（前端热加载 + Rust 热编译）
cargo tauri dev
```

### 构建发布版本

```bash
# 构建便携版 exe（仅生成裸 exe，不打包安装程序）
cargo tauri build
```

构建产物位于 `target/release/llm-api-proxy-app.exe`，发布时重命名为 `LLM-API-Proxy_vX.Y.Z_x64_portable.exe`。

## 📖 使用指南

### 1. 启动程序

双击运行 `.exe`，程序自动打开 GUI 窗口并在后台启动 API 网关服务（默认监听 `127.0.0.1:47339`）。Gateway API Key 在首次启动时自动生成，可在「网关设置」页面查看。

### 2. 添加上游订阅

进入「上游管理」页面，点击新增：

1. 填写供应商名称、Base URL、API Key
2. 点击「一键获取模型列表」拉取该厂商可用模型
3. 选择目标模型并保存

API Key 会被 AES-256-GCM 加密后存入数据库，明文不落盘。

### 3. 创建号池

进入「号池管理」页面，创建一个号池：

1. 填写号池名称（如 `grok-4.5`，即终端工具中看到的模型名）
2. 关联一个或多个上游订阅
3. 可选：开启思考模式、设置最大并发数、配置故障转移

### 4. 对接终端工具

在任意 OpenAI 兼容客户端中配置：

```
Base URL: http://127.0.0.1:47339/v1
API Key:  sk-gw-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
模型:     grok-4.5
```

即可开始使用。请求会自动在号池内的上游之间轮询，某家故障时自动切换。

### 5. 最小化到托盘

关闭窗口后程序最小化到系统托盘，后台继续提供代理服务。双击托盘图标或右键「打开主窗口」可恢复窗口。

### 6. 版本更新

程序启动时会自动检查 GitHub / Gitee 上的最新发布版本。发现新版本时：

1. 侧边栏检查更新按钮显示红色提示圆点
2. 进入「设置」页面，自动显示新版本信息和更新日志
3. 点击「GitHub 下载」或「Gitee 下载」按钮，程序自动下载新版本并替换重启
4. 更新完成后桌面快捷方式不受影响（自动刷新 Windows 图标缓存）

> 国内用户推荐使用 Gitee 下载以获得更快的下载速度。

## 💬 交流反馈

欢迎加入 QQ 群进行交流、问题反馈和功能建议：

![QQ群二维码](./images/qrcode_1785163779542.jpg)

## ⭐ Star 支持

如果本项目对你有帮助，请帮忙点个 Star ⭐，这将帮助更多人发现本项目，感谢你的支持！

## �� API 参考

### OpenAI 兼容端点

| 端点 | 方法 | 说明 |
|------|------|------|
| `/v1/models` | GET | 返回所有号池名称作为可用模型列表 |
| `/v1/chat/completions` | POST | 转发请求至号池上游，支持流式/非流式 |
| `/api/health` | GET | 健康检查 |

**请求示例：**

```http
POST /v1/chat/completions
Authorization: Bearer sk-gw-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
Content-Type: application/json

{
  "model": "grok-4.5",
  "messages": [{"role": "user", "content": "你好"}],
  "stream": true
}
```

### 故障转移行为

- 上游返回 HTTP 5xx 或连接超时 → 自动尝试下一个上游
- 上游返回 HTTP 200 但响应体包含 `error` 字段 → 同样触发故障转移
- 所有上游均失败 → 返回 `502 Bad Gateway`，响应体包含每个失败上游的错误详情
- 号池内无可用上游（全部禁用）→ 返回 `503 Service Unavailable`

## 🔒 安全设计

| 措施 | 说明 |
|------|------|
| 仅监听本地 | 默认绑定 `127.0.0.1`，不对外网开放 |
| API Key 加密存储 | 使用 AES-256-GCM 加密，Master Key 存于本地文件，明文不落盘 |
| 常量时间比较 | API Key 校验使用常量时间比较，防止时序侧信道攻击 |
| XSS 防护 | 前端动态内容输出经过 HTML 转义 |
| 命令注入防护 | 外部链接打开前校验 URL 协议白名单 |
| 事务隔离 | 数据库写操作使用事务 + Mutex 锁，防止并发干扰 |
| UUID 标识 | 请求日志使用 UUID v4 生成 ID，避免碰撞导致日志丢失 |

## 📦 便携部署

程序和数据全部跟随 exe 所在目录，无需安装运行时依赖：

```
LLM-API-Proxy/
├── LLM-API-Proxy.exe        # 主程序
└── data/                    # 所有数据跟随目录移动
    ├── proxy.db             # SQLite 数据库（配置 + 日志）
    └── master_key.bin       # AES 主密钥
```

- 复制整个文件夹到 U 盘，插到任意 Windows 电脑即可运行
- 删除文件夹即完全卸载，不写注册表、不注册系统服务

## 📝 许可证

本项目采用 **双轨许可模式**（Dual-License）。

### 默认开源许可：AGPL-3.0-only

本项目默认基于 [GNU AGPL-3.0](./LICENSE) 开源。你可以自由使用、修改和分发，但衍生作品必须以相同许可证开源，且通过网络提供服务时也必须公开源码。

### 商业授权

以下**服务式商业用途**须事先获得项目维护者的商业授权：

- 将本项目（或修改版）作为后端或核心服务，向第三方提供 SaaS、托管、管理等付费或免费服务
- 将本项目集成到商业产品中分发或销售
- 企业、组织、团队等非个人主体的内部生产使用

| 用途 | 许可要求 |
|------|----------|
| 个人学习 / 研究 / 日常使用 | ✅ AGPL-3.0 免费使用 |
| 修改后自用（不分发、不提供服务） | ✅ AGPL-3.0 免费使用 |
| 开源社区贡献（Issue / PR） | ✅ 欢迎贡献，详见 [CONTRIBUTING.md](./CONTRIBUTING.md) |
| 修改后分发（开源衍生作品） | ✅ 须以 AGPL-3.0 开源衍生作品 |
| 服务式商业使用（SaaS / 托管 / 企业内部） | ⚠️ 须取得商业授权 |

> **简而言之：个人和非商业用途可基于 AGPL-3.0 免费使用；如需将本项目用于商业服务，请提前联系获取授权。**
>
> 商业授权请联系项目维护者（通过 GitHub Issues）。

### 贡献者协议

提交 Pull Request 即表示你同意 [CLA.md](./CLA.md) 中的条款。你保留贡献的版权，但授权项目维护者可将其用于商业发行版。
