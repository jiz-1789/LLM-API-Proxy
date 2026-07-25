# LLM-API-Proxy

统一管理多厂商大模型 API 的本地代理服务，带原生桌面 GUI 窗口。

## 快速开始

### 环境要求

- Rust 1.77+ (推荐最新 stable)
- Tauri CLI `tauri-cli` 2.0+
- Node.js 18+
- Windows 10/11（MVP 阶段仅支持 Windows）

### 构建

```bash
# 1. 安装 Tauri CLI
cargo install tauri-cli --version "^2"

# 2. 构建并运行开发版本
cargo tauri dev

# 3. 构建发布版本（单文件 .exe）
cargo tauri build
```

### 项目结构

```
.
├── Cargo.toml              # workspace 根配置
├── src/                    # Rust 后端
│   ├── main.rs             # 入口
│   ├── lib.rs              # 模块声明 + AppState
│   ├── config.rs           # Gateway 配置
│   ├── crypto.rs           # AES-256-GCM 加密
│   ├── db.rs               # SQLite 数据层
│   ├── error.rs            # 统一错误类型
│   ├── gateway/            # OpenAI 兼容网关
│   │   ├── mod.rs          # /v1/models, /v1/chat/completions
│   │   └── auth.rs         # API Key 认证
│   ├── pool/               # 号池与轮询逻辑
│   │   ├── mod.rs          # PoolRouter trait
│   │   ├── round_robin.rs  # 顺序轮询
│   │   ├── circuit_breaker.rs # 熔断器
│   │   └── thinking.rs     # 思考模式参数注入
│   └── proxy/              # 上游转发
│       ├── mod.rs          # ProxyEngine
│       ├── client.rs       # UpstreamConfig 定义
│       ├── failover.rs     # HTTP 转发与故障转移
│       └── model_filter.rs # 模型名替换
├── src-tauri/              # Tauri 前端资源
│   ├── src/lib.rs          # Tauri 应用入口
│   └── tauri.conf.json     # 窗口/打包配置
└── data/                   # 运行时数据（随 exe 移动）
    ├── proxy.db            # SQLite
    └── master_key.bin      # AES Master Key
```

## 当前状态

- [x] Tauri v2 项目初始化完成
- [x] Workspace Cargo.toml 配置完成
- [x] 核心模块骨架代码编写完成
  - [x] `config.rs` — Gateway 配置
  - [x] `crypto.rs` — AES-256-GCM 加解密
  - [x] `db.rs` — SQLite CRUD
  - [x] `pool/round_robin.rs` — 轮询选择器
  - [x] `pool/circuit_breaker.rs` — 熔断器
  - [x] `pool/thinking.rs` — 思考模式参数映射
  - [x] `gateway/mod.rs` — OpenAI 兼容网关路由
  - [x] `gateway/auth.rs` — API Key 认证
  - [x] `proxy/failover.rs` — 上游转发
  - [x] `proxy/model_filter.rs` — 模型名替换
- [ ] Tauri 与后端 engine 集成
- [ ] GUI 前端页面开发
- [ ] 单文件打包部署
- [ ] 全链路测试

## 技术选型

| 模块 | 技术 |
|------|------|
| 后端框架 | Axum (Rust) |
| GUI | Tauri v2 |
| 数据库 | SQLite (rusqlite) |
| 加密 | AES-256-GCM |
| HTTP 客户端 | reqwest |
| 日志 | tracing + tracing-subscriber |

## 许可证

MIT
