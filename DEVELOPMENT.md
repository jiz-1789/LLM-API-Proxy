# 开发规范与数据库兼容性文档

> **核心原则：所有未来的开发必须兼容旧版本数据库，禁止任何会导致用户更新后数据丢失的变更。**

---

## 一、数据库迁移规范

### 1.1 迁移机制说明

项目使用 `schema_version` 表追踪数据库版本，迁移逻辑位于 `src/db.rs` 的 `run_migrations()` 方法中。

- `create_schema()` 创建所有表的**初始结构**（v1），使用 `CREATE TABLE IF NOT EXISTS`
- `run_migrations()` 按版本号顺序执行增量迁移
- 每个迁移执行成功后更新 `schema_version` 表

### 1.2 迁移规则（必须遵守）

| 规则 | 说明 |
|------|------|
| ✅ 只允许 `ALTER TABLE ADD COLUMN` | 新增列，不影响旧数据 |
| ✅ 新增列必须有 `DEFAULT` 值（除可空列） | 旧行自动填充默认值 |
| ✅ 新增列尽量用 `NULL` 或 `DEFAULT ''` / `DEFAULT 0` | 确保旧数据兼容 |
| ✅ 可以新建表 | 使用 `CREATE TABLE IF NOT EXISTS` |
| ✅ 可以新建索引 | 使用 `CREATE INDEX IF NOT EXISTS` |
| ❌ 禁止 `DROP COLUMN` | SQLite 支持有限且会丢数据 |
| ❌ 禁止 `DROP TABLE` 再重建 | 直接丢失所有数据 |
| ❌ 禁止修改列名或列类型 | 旧数据不兼容 |
| ❌ 禁止删除已有列 | 数据丢失 |

### 1.3 添加新迁移步骤

1. 在 `run_migrations()` 的 `migrations` 向量末尾追加新版本号
2. 版本号必须**严格递增 +1**，不可跳号
3. SQL 语句只使用 `ALTER TABLE ... ADD COLUMN` 或 `CREATE TABLE/INDEX IF NOT EXISTS`
4. 新增列如果是 `TEXT` 类型，根据语义选择：
   - 必须有值的字段：`NOT NULL DEFAULT ''`
   - 可选字段：不加 `NOT NULL`（允许 NULL）
5. 更新本文档底部的「迁移历史」表

**示例：**

```rust
(5, "ALTER TABLE request_logs ADD COLUMN latency_category TEXT;"),
```

### 1.4 需要修改列语义时的处理

如果需要改变某列的含义或类型，采用**新增列 + 数据回填**策略：

```rust
// 1. 新增列
(5, "ALTER TABLE upstreams ADD COLUMN api_key_v2 BLOB;"),
// 2. 代码层读取时兼容：优先读 v2，为空则读 v1 并自动迁移
```

**绝对不能**删除旧列。代码层做双读兼容，新写入写新列。

---

## 二、当前数据库 Schema（v4）

### 2.1 `schema_version`

| 列 | 类型 | 说明 |
|----|------|------|
| version | INTEGER | 当前 schema 版本号 |

### 2.2 `upstreams` — 上游服务商

| 列 | 类型 | 约束 | 迁移版本 | 说明 |
|----|------|------|----------|------|
| id | TEXT | PRIMARY KEY | v1 | UUID |
| provider_name | TEXT | NOT NULL | v1 | 厂商名 |
| base_url | TEXT | NOT NULL | v1 | API 基础地址 |
| api_key_encrypted | BLOB | NOT NULL | v1 | AES-256-GCM 加密后的密钥 |
| selected_model | TEXT | NOT NULL | v1 | 当前选中的模型 |
| enabled | INTEGER | NOT NULL DEFAULT 1 | v1 | 是否启用 (0/1) |
| remark | TEXT | DEFAULT '' | v1 | 备注 |
| status | TEXT | NOT NULL DEFAULT 'healthy' | v1 | 健康状态 |
| failure_count | INTEGER | NOT NULL DEFAULT 0 | v1 | 连续失败次数 |
| last_failure_time | TEXT | | v1 | 最后失败时间 |
| created_at | TEXT | NOT NULL DEFAULT datetime('now') | v1 | |
| updated_at | TEXT | NOT NULL DEFAULT datetime('now') | v1 | |
| available_models | TEXT | NOT NULL DEFAULT '[]' | v2 | 可用模型列表 (JSON 数组) |

### 2.3 `pools` — 号池

| 列 | 类型 | 约束 | 迁移版本 | 说明 |
|----|------|------|----------|------|
| id | TEXT | PRIMARY KEY | v1 | UUID |
| name | TEXT | NOT NULL UNIQUE | v1 | 内部名称 |
| display_name | TEXT | NOT NULL | v1 | 显示名称 |
| round_robin_strategy | TEXT | NOT NULL DEFAULT 'sequential' | v1 | 轮询策略 |
| failover_enabled | INTEGER | NOT NULL DEFAULT 1 | v1 | 故障转移开关 |
| timeout_seconds | INTEGER | NOT NULL DEFAULT 30 | v1 | 超时时间 |
| max_concurrency | INTEGER | NOT NULL DEFAULT 5 | v1 | 最大并发 |
| thinking_enabled | INTEGER | NOT NULL DEFAULT 0 | v1 | 思考模式开关 |
| circuit_breaker_threshold | INTEGER | NOT NULL DEFAULT 3 | v1 | 熔断阈值 |
| circuit_breaker_duration_seconds | INTEGER | NOT NULL DEFAULT 60 | v1 | 熔断持续时间 |
| created_at | TEXT | NOT NULL DEFAULT datetime('now') | v1 | |
| updated_at | TEXT | NOT NULL DEFAULT datetime('now') | v1 | |

### 2.4 `pool_upstreams` — 号池-上游关联

| 列 | 类型 | 约束 | 迁移版本 | 说明 |
|----|------|------|----------|------|
| pool_id | TEXT | NOT NULL, FK→pools(id) ON DELETE CASCADE | v1 | |
| upstream_id | TEXT | NOT NULL, FK→upstreams(id) ON DELETE CASCADE | v1 | |
| sort_order | INTEGER | NOT NULL DEFAULT 0 | v1 | 排序权重 |
| model | TEXT | NOT NULL DEFAULT '' | v2 | 该上游在此号池中使用的模型 |

主键：`(pool_id, upstream_id)`

### 2.5 `settings` — 键值设置

| 列 | 类型 | 约束 | 说明 |
|----|------|------|------|
| key | TEXT | PRIMARY KEY | |
| value | TEXT | NOT NULL | |
| updated_at | TEXT | NOT NULL DEFAULT datetime('now') | |

### 2.6 `request_logs` — 请求日志

| 列 | 类型 | 约束 | 迁移版本 | 说明 |
|----|------|------|----------|------|
| id | TEXT | PRIMARY KEY | v1 | |
| request_id | TEXT | NOT NULL | v1 | |
| pool_name | TEXT | | v1 | |
| upstream_id | TEXT | | v1 | |
| failed_upstreams | TEXT | DEFAULT '[]' | v1 | 失败的上游列表 (JSON) |
| method | TEXT | NOT NULL | v1 | |
| endpoint | TEXT | NOT NULL | v1 | |
| status_code | INTEGER | NOT NULL | v1 | |
| response_time_ms | INTEGER | NOT NULL | v1 | |
| is_streaming | INTEGER | NOT NULL DEFAULT 0 | v1 | |
| created_at | TEXT | NOT NULL DEFAULT datetime('now') | v1 | |
| prompt_tokens | INTEGER | NOT NULL DEFAULT 0 | v3 | 输入 Token |
| completion_tokens | INTEGER | NOT NULL DEFAULT 0 | v3 | 输出 Token |
| total_tokens | INTEGER | NOT NULL DEFAULT 0 | v3 | 总 Token |
| model | TEXT | (NULL) | v4 | 请求使用的模型名 |

**索引：**
- `idx_request_logs_created_at` — `created_at`
- `idx_request_logs_pool_name` — `pool_name`

---

## 三、迁移历史

| 版本 | 说明 | 涉及表 | 变更内容 |
|------|------|--------|----------|
| v1 | 初始 Schema | 全部 | 创建所有基础表和索引 |
| v2 | 模型列表 + 号池模型映射 | upstreams, pool_upstreams | `upstreams.available_models TEXT NOT NULL DEFAULT '[]'`<br>`pool_upstreams.model TEXT NOT NULL DEFAULT ''` |
| v3 | Token 统计 | request_logs | `request_logs.prompt_tokens INTEGER NOT NULL DEFAULT 0`<br>`request_logs.completion_tokens INTEGER NOT NULL DEFAULT 0`<br>`request_logs.total_tokens INTEGER NOT NULL DEFAULT 0` |
| v4 | 模型维度统计 | request_logs | `request_logs.model TEXT` (可空) |

---

## 四、代码层兼容性规范

### 4.1 读取旧数据的兼容策略

新增的可空列在读取时必须处理 `NULL`：

```rust
// ✅ 正确：使用 COALESCE 或 Rust 层 unwrap_or
let model: Option<String> = row.get(4)?;
let model = model.unwrap_or_else(|| "未记录".to_string());

// ✅ 正确：SQL 层 COALESCE
"SELECT COALESCE(NULLIF(model, ''), '未记录') as model, ..."
```

### 4.2 写入时的兼容策略

新增列的写入逻辑应做条件判断，确保不会因为列不存在而 panic：

- 迁移是幂等的（`IF NOT EXISTS`）
- 代码层的 INSERT 语句必须包含所有列（包括新增列）
- 对于可选列，传 `None` 或空值

### 4.3 重置/清理操作的边界

- `clear_logs()` — 删除 `request_logs` 全表，不影响其他表
- `reset_upstream_token_stats(upstream_id)` — 只删除指定上游的日志，不影响其他上游

---

## 五、开发流程检查清单

每次涉及数据库变更时，提交前确认：

- [ ] 迁移版本号 +1，未跳号
- [ ] 只使用 `ALTER TABLE ADD COLUMN` 或 `CREATE TABLE/INDEX IF NOT EXISTS`
- [ ] 新增 `NOT NULL` 列有 `DEFAULT` 值
- [ ] 代码层的 `map_*_row` 函数已更新列索引
- [ ] 代码层的 `INSERT` 语句已更新包含新列
- [ ] 代码层读取新列时处理了 `NULL` / 空值情况
- [ ] 单元测试中的 `insert_*` 调用已更新参数
- [ ] 本文档的 Schema 表和迁移历史已更新
- [ ] 在旧数据库上测试过升级路径（不会报错、不丢数据）
