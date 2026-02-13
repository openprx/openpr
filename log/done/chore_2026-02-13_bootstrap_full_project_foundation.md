# chore: 完整项目基础工程落地（一次性初始化）

## 背景
- 诉求：减少逐条沟通，直接完成“可开发可扩展”的项目基线。
- 约束：遵循 Rust 2024 + Axum + SeaORM + PostgreSQL + Docker 技术路线。

## 变更点
- 工程结构：
  - 新增 workspace 根配置（`Cargo.toml`、`rust-toolchain.toml`、`.cargo/config.toml`）。
  - 新增三服务：`apps/api`、`apps/worker`、`apps/mcp-server`。
  - 新增共享基础库：`crates/platform`。
- 运行基础：
  - 新增 `Dockerfile` 与 `docker-compose.yml`。
  - 新增 `.env.example`、`scripts/dev-up.sh`、`scripts/dev-check.sh`。
- 数据基线：
  - 新增 `migrations/0001_init.sql`，覆盖业务表、`cache_entries`、`job_queue`、`scheduled_jobs`。
- 文档基线：
  - 新增 `README.md` 与 `docs/architecture/BASELINE_IMPLEMENTATION_PLAN.md`。

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
- `log/task/task_2026-02-13_foundation_bootstrap_full_project.md`
- `log/changelog.md`

## 验证命令与结果
- 命令：`cargo check --workspace --all-targets`
- 结果：失败，当前执行环境缺少 `cargo`（`/bin/bash: cargo: command not found`）。
- 命令：`find . -maxdepth 4 -type f | sort`
- 结果：目标工程文件全部已创建。

## 后续建议
1. 在具备 Rust toolchain 的环境执行 `./scripts/dev-check.sh`。
2. 先实现 API 核心资源（workspace/project/work_item）与对应 DB 迁移演进。
3. 并行推进 MCP tool 实现与前端 MVP 初始化。
