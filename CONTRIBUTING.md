# Contributing

感谢你参与 LLM-API-Proxy 的开发！欢迎提交 Issue、Pull Request 和建议。

## 贡献许可协议

当你有意向本仓库提交代码、提示词、测试、文档、素材或其他内容时，即表示你已阅读并同意 [CLA.md](./CLA.md)，包括以下核心条款：

- 你有权合法提交该贡献；
- 你同意该贡献可作为本项目的一部分，按 GNU Affero General Public License v3.0 only (AGPL-3.0-only) 分发；
- 你同意项目维护者可将该贡献纳入另行授权的商业发行版或商业授权中；
- 你会在 Pull Request 描述或相关讨论中明确标注任何第三方素材及其许可证义务。

如果你需要在不同条款下提交贡献，请在提交前先与项目维护者沟通并取得明确书面同意。

## 第三方素材

如果你的贡献包含第三方代码、素材或数据，请同时提供：

- 原始来源；
- 适用的许可证；
- 需要添加到 NOTICE 或其他仓库元数据中的归属或声明文本。

## 开发规范

提交代码前请阅读 [AGENTS.md](./AGENTS.md)，其中包含：

- 数据库迁移规范（最高优先级，禁止破坏性变更）
- Rust 代码风格与约定
- 安全规范（API Key 处理、XSS 防护等）
- 网关与代理逻辑规范
- 测试规范与提交前检查清单

## 提交流程

1. Fork 本仓库
2. 基于 `main` 创建功能分支：`feature/your-feature`
3. 编写代码并确保通过以下检查：
   - `cargo build` 编译通过
   - `cargo clippy` 无 WARNING / ERROR
   - `cargo test` 全部通过
4. 提交代码，Commit Message 遵循 Conventional Commits 规范（详见 AGENTS.md 第七章）
5. 创建 Pull Request，描述变更内容和动机

## Commit Message 规范

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
| `chore` | 杂项 |

**示例：**

```
feat: 新增上游批量导入功能
fix: 修复流式响应 model 字段未替换的问题
perf: 优化 SSE chunk 处理避免二次 JSON 解析
```

## 提交粒度

- **一任务一 commit**：每个功能点或修复独立提交
- **原子性**：一个 commit 只做一件事
- **可回滚**：每个 commit 可独立 revert
