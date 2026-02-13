# 任务：完整项目基础落地（Rust Workspace + API/Worker/MCP + PostgreSQL + Docker）

## 上下文/动机
- 目标：从规划进入可执行代码基线，避免继续停留在纯文档阶段。
- 要求：按既定技术栈（Rust 2024/Axum/SeaORM/PostgreSQL/Tokio/Tracing/Docker）一次性完成基础工程落地。

## 结论与方案
- 采用 monorepo workspace：`apps/api`、`apps/worker`、`apps/mcp-server`、`crates/platform`。
- 统一基础能力：配置加载、日志初始化、数据库连接、错误类型。
- PostgreSQL 一库多职能：业务表 + 缓存表 + 队列表 + 定时任务表。
- Docker 驱动：通过 `docker-compose` 一键拉起 `postgres` + 三个服务容器。

## 涉及文件
- `Cargo.toml`
- `rust-toolchain.toml`
- `.cargo/config.toml`
- `.gitignore`
- `.env.example`
- `Dockerfile`
- `docker-compose.yml`
- `README.md`
- `scripts/dev-up.sh`
- `scripts/dev-check.sh`
- `migrations/0001_init.sql`
- `apps/api/Cargo.toml`
- `apps/api/src/main.rs`
- `apps/worker/Cargo.toml`
- `apps/worker/src/main.rs`
- `apps/mcp-server/Cargo.toml`
- `apps/mcp-server/src/main.rs`
- `crates/platform/Cargo.toml`
- `crates/platform/src/lib.rs`
- `crates/platform/src/app.rs`
- `crates/platform/src/config.rs`
- `crates/platform/src/error.rs`
- `crates/platform/src/logging.rs`
- `docs/architecture/BASELINE_IMPLEMENTATION_PLAN.md`
- `log/done/chore_2026-02-13_bootstrap_full_project_foundation.md`
- `log/changelog.md`

## 计划
- [ ] 补充 SeaORM entity + migration 管理工具链
- [ ] 完成 API v1 核心资源路由（workspace/project/work_item）
- [ ] 完成 worker 队列轮询与 scheduler 执行
- [ ] 完成 MCP tools 与应用服务绑定
- [ ] 前端应用初始化并接入 API

## 已完成
- [x] 创建 Rust workspace 与统一依赖配置（edition 2024）。
- [x] 创建 API/Worker/MCP 三个可启动入口骨架。
- [x] 创建共享 `platform` crate（配置/日志/DB/错误）。
- [x] 创建 PostgreSQL 初始化迁移（业务+缓存+队列+定时任务）。
- [x] 创建 Dockerfile 与 docker-compose 本地编排。
- [x] 创建 README、脚本与架构基线文档。

## 未完成
- [ ] 在当前环境执行 `cargo` 系列校验（环境中缺少 cargo）。
- [ ] 业务功能层（RBAC、CRUD、搜索、Webhook、MCP真实工具）尚待实现。
- [ ] 前端代码尚未开始落地。

## 对应 done/ 文档索引
- `log/done/chore_2026-02-13_bootstrap_full_project_foundation.md`
