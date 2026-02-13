# Rust 开发规范（Rust Coding Standard）

> 版本：v1.0  
> 最后更新：2026-02-10  
> 适用范围：本仓库所有 Rust 代码（`src/`、`crates/*/src/` 等），除非某条规则明确说明例外目录（如 `tests/`）  
> 目标：可维护、可观测、可演进、可上线的 Rust 工程实践（尤其适用于服务端/工具链/数据处理）

## 0. 规则等级与基本原则

### 0.1 规则等级
- **MUST**：必须遵守。违反即不允许合并。
- **SHOULD**：强烈建议。若不遵守，需在 PR 描述或代码注释写明理由。
- **MAY**：可选建议，视具体场景采用。

### 0.2 基本原则（MUST）
- **不依赖 panic 作为控制流**：生产代码不应因可预期输入/外部错误而崩溃。
- **所有外部交互都可失败**：文件、网络、数据库、环境变量、序列化、锁等都必须显式处理错误。
- **质量门槛自动化**：所有强制规则都尽量由 CI/Lint 工具强制执行，而不是靠人工记忆。

## 1. 工具链与仓库约定

### 1.1 Rust 版本与工具链（MUST）
- 仓库必须提供 `rust-toolchain.toml`，固定 CI 与开发机使用一致工具链（避免“我这能编译你那不行”）。
- 必须明确最小支持 Rust 版本（MSRV），写在：
  - `README.md` 或 `CONTRIBUTING.md`
  - 或 `rust-toolchain.toml` 注释中
- 若升级 MSRV：必须在变更说明中写清楚原因与影响范围。

### 1.2 格式化与 Lint（MUST）
- 必须通过 `rustfmt` 与 `clippy`（见第 12 节 CI 清单）。
- 代码风格以 `rustfmt` 输出为准，不接受手工“对齐美化”导致的风格偏差。

### 1.3 工程结构（SHOULD）
- 业务/领域逻辑与基础设施（DB/HTTP/消息队列）分层，避免在 handler 中堆积复杂逻辑。
- 对外 API（库的 `pub`）与内部实现分离：`pub` 尽量稳定、少暴露实现细节。

## 2. 禁止项（MUST NOT）

### 2.1 禁止的宏/调用（MUST NOT）
以下内容在**生产代码**（`src/`、`crates/*/src/`）中一律禁止：
- `unwrap()`
- `expect()`
- `todo!()`
- `unimplemented!()`

**例外（MAY）**：仅允许出现在：
- `tests/`、`benches/`、`examples/` 中
- 且建议加一句注释说明为何可接受（例如“测试数据保证存在”）。

### 2.2 禁止未使用项（MUST NOT）
- 禁止未使用变量、参数、导入、特性开关（features）。
- 禁止用 `#[allow(unused_*)]` 作为逃生通道（除非规则明确允许）。
- **若确实需要保留但暂未使用（SHOULD）**：
  - 变量/参数以 `_` 前缀命名（例如 `_ctx`、`_req`）
  - 导入改为按需导入；不要用 `use xxx::*;` 掩盖未使用

### 2.3 禁止调试输出进入生产代码（MUST NOT）
生产代码禁止：
- `dbg!`
- `println!` / `eprintln!`

必须使用结构化日志（见第 7 节 `tracing`）。

### 2.4 禁止“占位实现”进入主分支（MUST NOT）
- 禁止提交“假设性/占位实现”：例如空实现、返回固定值、忽略参数等以“先过编译”为目的的代码。
- 允许存在 TODO 注释（MAY），但必须满足：
  - 指向 issue/任务编号（例如 `TODO(#123): ...`）
  - 不能影响功能正确性与测试覆盖（即：TODO 只能是优化/重构，不是未完成核心逻辑）

## 3. 错误处理规范

### 3.1 基本要求（MUST）
- 所有可失败操作必须返回 `Result`，并使用 `?` 传播或显式处理：
  - 文件/网络/DB/序列化/解析/锁/通道发送/JoinHandle 等
- 不允许静默吞错：
  - 禁止 `if let Err(_) = ... {}`
  - 禁止 `let _ = fallible_call();` 用于压掉 `must_use`（除非确有理由并写注释）
- 不允许丢失根因：
  - 禁止把具体错误直接替换成无信息的自定义错误（如 `map_err(|_| MyError)`）

### 3.2 错误类型策略（MUST）
- **库 crate（library）**：使用结构化错误类型（推荐 `thiserror`）
  - 要求：可匹配、可区分、可保留 `source`
- **应用/服务（binary）**：
  - 边界层（如 `main`、顶层任务/HTTP 入口）可以使用 `anyhow::Result` 聚合错误
  - 业务层/领域层仍建议使用结构化错误，便于映射错误码

### 3.3 上下文补充（SHOULD）
- 在跨边界调用（读配置、HTTP、DB）时，为错误增加上下文，便于定位：
  - `anyhow::Context`（应用层）
  - 或自定义错误变体携带关键字段（库/业务层）

### 3.4 错误映射与对外返回（MUST）
- 对外接口（HTTP/RPC/CLI）必须将内部错误映射为稳定的错误码/状态码：
  - 日志中保留完整错误链
  - 返回给用户的信息必须可控，不泄露敏感信息与内部实现细节（见第 10 节）

## 4. 所有权/借用与性能

### 4.1 避免不必要拷贝（MUST）
- 在热路径禁止无理由的 `clone()` / `to_string()` / `collect()`。
- 若必须拷贝（SHOULD）：
  - 在代码附近写明原因（例如生命周期/线程边界/缓存键需要拥有所有权）
  - 或用基准测试证明收益（见第 8 节）

### 4.2 API 设计优先借用（SHOULD）
- 优先接收借用类型：
  - `&str`、`&[u8]`、`&Path` / `impl AsRef<Path>`
- 需要“可借用也可拥有”时可用：
  - `Cow<'a, str>` / `Cow<'a, [u8]>`（谨慎使用，避免过度复杂）

### 4.3 集合与迭代（SHOULD）
- 优先迭代器与切片，避免中间集合。
- 对于大数据处理：
  - 避免在循环内频繁分配（可复用 buffer）
  - 关注 `O(n^2)` 的隐藏成本（如重复 `Vec::remove(0)`）

## 5. 并发与异步（Tokio/Async）

### 5.1 异步代码禁止阻塞（MUST）
- 在 async 上下文禁止直接调用阻塞操作：
  - 阻塞 IO（同步文件读写）
  - 重 CPU 计算
  - 同步网络请求
- 处理方式（SHOULD）：
  - 使用 `tokio::fs`、异步客户端
  - CPU/阻塞工作用 `tokio::task::spawn_blocking` 或专用线程池

### 5.2 超时与取消（MUST）
- 所有外部调用（DB/HTTP/消息队列）必须具备超时策略：
  - 单次调用超时
  - 或整体请求/任务超时
- 对可取消的任务：必须正确传播取消（例如使用 `select!` 并及时退出）

### 5.3 并发上限（SHOULD）
- 批量并发必须设置上限：
  - `Semaphore` / worker pool / `buffer_unordered(n)`
- 禁止无界队列在生产路径无约束堆积（除非能证明不会积压，并在注释/文档写明）。

### 5.4 任务管理（SHOULD）
- `tokio::spawn` 的任务必须被“观察”：
  - 要么 `await` JoinHandle
  - 要么集中管理（如 `JoinSet`），并记录失败

## 6. 数据库与 SQLx 规范

### 6.1 SQLx 限制（MUST）
- 避免使用编译期宏：
  - 禁止 `sqlx::query!` / `query_as!`
- 必须使用运行时接口：
  - `sqlx::query(...)` / `sqlx::query_as::<_, T>(...)` + `.bind(...)`

### 6.2 参数绑定与注入防护（MUST）
- 禁止 SQL 字符串拼接构造参数：
  - 例如 `format!("... {}", user_input)` 这类做法禁止（SQL 注入风险）
- 一律使用参数化查询与 `.bind(...)`。

### 6.3 查询约定（MUST）
- 禁止 `SELECT *`，必须显式列名（减少 schema 变更隐患）。
- 行数语义必须明确：
  - 可能没有：用 `fetch_optional`
  - 必须存在：用 `fetch_one`，并将 `RowNotFound` 映射为业务错误
- 多步写操作必须在事务中（除非明确说明无需事务的原因）。

### 6.4 SQL 组织方式（SHOULD）
- 复杂 SQL 建议放在 `*.sql` 文件中，通过 `include_str!()` 引入：
  - 便于审查、格式化、复用
- SQL 文件命名与模块/函数对应，避免散落难以追踪。

### 6.5 连接池与超时（SHOULD）
- 连接池必须配置：
  - 最大连接数、最小空闲、获取连接超时、空闲回收等
- 必须具备查询/事务超时策略（项目级统一）。

### 6.6 数据库迁移与回归（SHOULD）
- schema 变更必须通过 migrations 管理。
- 建议 CI 包含：
  - 迁移可执行
  - 关键查询集成测试（见第 8 节）

## 7. 日志与可观测性

### 7.1 统一使用 `tracing`（MUST）
- 禁止 `println!`/`dbg!`（见第 2 节）。
- 使用 `tracing::{info, warn, error, debug, trace}` 记录结构化字段：
  - 请求/任务 ID（trace id）
  - 关键参数（脱敏）
  - 耗时（DB/外部调用/关键函数）

### 7.2 错误日志（MUST）
- 记录错误时必须包含：
  - 错误本身
  - 关键上下文字段
- 对可预期的业务错误：
  - 不要用 `error!` 造成噪音（根据规范选择 `warn!` 或 `info!`）

### 7.3 敏感信息（MUST）
- 禁止日志输出敏感信息：
  - 密码、token、私钥、会话、身份证号、完整银行卡号等
- 必要时只输出：
  - 哈希/截断值/最后 4 位等

## 8. 测试与质量保障

### 8.1 最低要求（MUST）
- 必须通过 `cargo test`
- 核心逻辑必须有单元测试（至少覆盖关键分支与边界条件）

### 8.2 集成测试（SHOULD）
- 对外边界（DB/HTTP/文件系统）建议有集成测试：
  - 数据库：真实跑 migrations + 关键 SQL 回归
- 测试必须可重复：
  - 不依赖墙钟时间与外部不稳定网络（除非明确隔离为“非阻塞”测试组）

### 8.3 基准与性能回归（MAY）
- 对性能敏感路径建议添加 `criterion` 基准测试。
- 若引入显著复杂度的优化（缓存/并发），建议配套基准或压测说明。

## 9. 依赖治理与供应链安全

### 9.1 依赖新增规则（SHOULD）
- 新增依赖必须说明：
  - 用途与替代方案
  - 是否启用默认 feature（尽量最小化）
- 禁止无理由引入“大而全”依赖。

### 9.2 安全扫描（MAY，但推荐）
- 建议 CI 增加：
  - `cargo audit` 或等价安全公告检查
  - `cargo deny`（license/advisories/bans/source）

### 9.3 许可证与合规（MUST，若有合规要求）
- 若项目有明确的许可证策略，必须保证新增依赖许可兼容。

## 10. 安全与敏感信息管理

### 10.1 配置与密钥（MUST）
- 密钥/凭据只能来自：
  - 环境变量
  - 外部配置文件（不入库）
  - 专用密钥管理系统（如有）
- 禁止提交真实密钥到仓库（包括历史提交）。

### 10.2 输入校验（MUST）
- 对外输入（HTTP 参数、CLI 参数、消息体、DB 外来数据）必须校验：
  - 类型校验
  - 长度/范围校验
  - 语义约束（如枚举值）

## 11. 允许的例外与 `allow` 策略

### 11.1 `#[allow(...)]`（SHOULD 严格限制）
- 原则：**能修就修，不能修才 allow**。
- 允许 `allow` 的典型场景：
  - 第三方生成代码（最好隔离到单独模块）
  - 平台差异导致的特定 lint 误报
- 每个 `allow` 必须附注释说明原因与范围。

## 12. 本地验证与 CI 清单（MUST）

### 12.1 必须通过（MUST）
```sh
cargo fmt -- --check
cargo check
cargo clippy -- -D warnings
cargo test
```

### 12.2 推荐增强（MAY）
```sh
# 依赖安全（任选其一或同时）
cargo audit
cargo deny check

# 文档（库项目推荐）
cargo doc --no-deps

# 发布前构建（可选）
cargo build --release
```

## 13. Code Review Checklist（评审清单）

评审时至少检查：
- 是否出现 unwrap/expect/todo/unimplemented（生产代码）
- 是否存在未使用导入/变量/参数
- 错误是否被吞掉？是否丢失根因？
- 是否对外输入做了校验？
- 是否在 async 中做了阻塞操作？
- 外部调用是否有超时与并发上限？
- SQL 是否参数化 `.bind`？是否避免 `SELECT *`？
- `fetch_one`/`fetch_optional` 语义是否正确？
- 日志是否使用 tracing？是否泄漏敏感信息？
- 是否补充了必要测试（尤其是边界条件）？
- 是否引入了不必要依赖或过多默认 feature？

## 附录 A：推荐配置样例

### A.1 `rust-toolchain.toml`（示例）
```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

### A.2 `.cargo/config.toml`（可选：命令别名）
```toml
[alias]
lint = "clippy -- -D warnings"
fmtc = "fmt -- --check"
check-all = "check"
test-all = "test"
```

### A.3 crate 根 Lint（示例）
在 `src/lib.rs` 或 `src/main.rs` 顶部添加（按项目需要增减）：

```rust
#![deny(warnings)]
#![deny(unused_imports, unused_variables)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![deny(clippy::todo, clippy::unimplemented)]
#![deny(clippy::dbg_macro, clippy::print_stdout, clippy::print_stderr)]
```

说明：`#![deny(warnings)]` 会把编译警告直接升级为错误，配合 CI 更严格。若项目需要更柔性，可仅依赖 CI 的 `clippy -D warnings`。

### A.4 错误类型（thiserror）示例
```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("database error")]
    Db(#[from] sqlx::Error),

    #[error("not found: {entity}")]
    NotFound { entity: &'static str },

    #[error("invalid input: {field}")]
    InvalidInput { field: &'static str },
}

pub type AppResult<T> = Result<T, AppError>;
```

### A.5 SQLx（运行时 `query_as + bind`）示例
```rust
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub email: String,
}

pub async fn load_user(pool: &PgPool, id: Uuid) -> Result<Option<UserRow>, sqlx::Error> {
    let user = sqlx::query_as::<_, UserRow>(
        "SELECT id, email FROM users WHERE id = $1"
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(user)
}
```

### A.6 明确区分“可能不存在”和“必须存在”示例
```rust
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::model::UserRow;

pub async fn require_user(pool: &PgPool, id: Uuid) -> AppResult<UserRow> {
    let user = sqlx::query_as::<_, UserRow>(
        "SELECT id, email FROM users WHERE id = $1"
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::from)?;

    match user {
        Some(u) => Ok(u),
        None => Err(AppError::NotFound { entity: "user" }),
    }
}
```

### A.7 `tracing`（结构化日志）示例
```rust
use tracing::{info, instrument};

#[instrument(skip(payload))]
pub async fn handle_request(request_id: &str, payload: &[u8]) -> Result<(), crate::error::AppError> {
    info!(%request_id, payload_len = payload.len(), "request received");

    // ...业务逻辑

    Ok(())
}
```

## 附录 B：提交规范建议（可选）
- 一个 PR 只解决一个问题域（避免大杂烩变更）。
- 变更包含 DB schema/SQL：
  - 必须附 migrations
  - 必须附关键查询/路径的测试或验证说明
- 若引入新依赖：
  - 必须说明原因与 feature 选择策略
