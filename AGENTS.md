# AGENTS.md - OpenPR 项目规范

## 目录结构

```
openpr/
├── apps/                   # 应用程序
│   ├── api/                # 后端 API (Rust + Axum)
│   ├── worker/             # 后台任务 Worker
│   └── mcp-server/         # MCP Server (12 tools)
├── crates/
│   └── platform/           # 共享库 (config, models)
├── frontend/               # 前端 (Svelte + SvelteKit + Bun)
├── migrations/             # 数据库迁移文件
├── log/                    # ⚠️ 不纳入 git（已加入 .gitignore）
│   ├── changelog.md        # 变更日志
│   ├── task/               # 任务规划文档（开发前的 spec）
│   ├── done/               # 已完成任务归档（含 Phase 报告）
│   ├── docs/               # 项目文档（API 文档、部署指南、设计文档等）
│   └── audit/              # 审计记录
├── Cargo.toml              # Rust workspace 配置
├── docker-compose.yml      # 容器编排
├── Dockerfile              # 完整构建镜像
├── Dockerfile.prebuilt     # 预编译二进制镜像
├── CHANGELOG.md            # 对外变更日志
├── CONTRIBUTING.md         # 贡献指南
└── README.md               # 项目说明
```

## 文件存放规则

### ✅ 根目录只放
- 构建配置：`Cargo.toml`, `Cargo.lock`, `Dockerfile*`, `docker-compose.yml`
- 项目元文件：`README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`
- 环境配置：`.env`, `.env.example`, `.gitignore`, `rust-toolchain.toml`

### ❌ 根目录禁止放
- Codex/子进程生成的报告文件（→ `log/docs/`）
- Phase 完成报告（→ `log/done/`）
- 任务规划文档（→ `log/task/`）
- 临时文件、备份文件

### 📁 log/ 目录分类规则
| 目录 | 用途 | 示例 |
|------|------|------|
| `log/task/` | 待执行的任务规划 | `task_2026-02-16_xxx.md` |
| `log/done/` | 已完成的任务归档 | `fix_2026-02-16_xxx.md` |
| `log/docs/` | 项目文档和设计稿 | `governance-protocol.md` |
| `log/audit/` | 审计和安全检查 | `audit_2026-02-16.md` |
| `log/changelog.md` | 内部变更记录 | — |

### ⚠️ 注意
- `log/` 目录不纳入 git（`.gitignore` 已配置）
- 子进程生成的报告必须放入 `log/docs/` 或 `log/done/`
- 不要在根目录创建任何 `*_REPORT.md`、`*_SUMMARY.md` 文件

## 部署规范

### 容器名称（podman）
| 服务 | 容器名 | 端口 |
|------|--------|------|
| PostgreSQL | openpr-postgres | 5432 |
| API | openpr-api | 8081→8080 |
| Worker | openpr-worker | — |
| MCP Server | openpr-mcp-server | 8090 |
| Frontend | openpr-frontend | 3000→80 |
| Webhook | openpr-webhook | 9090 (host) |

### Nginx 配置要点
- 使用 `container_name`（如 `openpr-api`），不用 compose service name
- 必须配置 `resolver 10.89.3.1 valid=10s`
- upstream 用 `set $variable` 方式引用，防止 DNS 缓存导致 502

### 部署后检查
1. `podman exec openpr-frontend nginx -s reload` — 刷新 Nginx DNS
2. `curl http://localhost:3000/api/v1/health` — 验证 API 代理
3. 前端部署后不需要重启容器，`podman cp` + `nginx -s reload` 即可
