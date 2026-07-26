# 更新日志

本项目所有重要变更均会记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### 新增
- 设置页新增「版本更新」区域，支持检查 GitHub 最新发布版本
- 使用教程新增「版本更新」章节，说明便捷版手动下载更新流程
- 仪表盘「最近活动」区域改为展示真实请求日志，替代硬编码占位文本

### 变更
- AGENTS.md 完善分支策略与版本发布规范（7.3 ~ 7.5 节）
- 侧边栏轮播图内容由广告替换为项目宣传语

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

[Unreleased]: https://github.com/jiz-1789/LLM-API-Proxy/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/jiz-1789/LLM-API-Proxy/releases/tag/v0.1.0
