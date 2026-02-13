# 任务：Plane（开源能力）Rust 复刻 + MCP 接入 全量开发清单

## 上下文/动机
- 目标：复刻 Plane 的核心开源功能（不含商业版），并新增 MCP Server 供 AI 模型调用。
- 技术约束：Rust `edition = "2024"`；`axum` 最新；`SeaORM` 最新；日志 `tracing`；错误 `thiserror + anyhow`；`PostgreSQL` 同时承载存储、缓存、队列、定时任务；Docker 驱动部署。
- 产出诉求：形成可执行、可追踪、可分阶段验收的开发任务清单，覆盖业务逻辑与平台能力。

## 结论与方案
- 采用“单体模块化 + 清晰分层”架构：`interfaces(http,mcp,worker)` / `application` / `domain` / `infrastructure`。
- 采用 PostgreSQL 一库多职能：
  - 存储：业务主表 + 审计表 + 配置表。
  - 缓存：`cache_entries`（TTL + 版本戳，按需失效）。
  - 队列：`job_queue`（`FOR UPDATE SKIP LOCKED` 拉取）。
  - 定时任务：`scheduled_jobs`（cron/interval + next_run_at）。
- MCP 与 REST 共用 Application Service，避免逻辑分叉。
- 范围明确：只做开源能力（项目、工作项、周期、模块、页面、通知、Webhook、基础分析、导入导出）+ MCP，不做商业能力（SSO/SAML/SCIM/高级治理）。

## 涉及文件
- `log/task/task_2026-02-13_plane-rust-mcp_full_delivery_plan.md`
- `log/done/chore_2026-02-13_create_plane_full_task_checklist.md`
- `log/changelog.md`

## 计划
1. 架构与工程基建
- [ ] 建立 workspace（`apps/api`、`apps/worker`、`apps/mcp-server`、`crates/*`）
- [ ] 接入配置系统（环境变量 + 配置文件）
- [ ] 接入 tracing（json + env-filter + request-id）
- [ ] 统一错误模型（domain error + transport mapping）
- [ ] Dockerfile、docker-compose、健康检查与就绪探针

2. 身份与组织域
- [ ] 用户、工作区、成员、角色模型
- [ ] API token / 机器人账号（为 MCP 与自动化服务）
- [ ] 权限中间件（workspace/project scoped ACL）

3. 项目与工作项域（核心）
- [ ] Project / Teamspace 实体与 API
- [ ] Work Item（类型、状态、优先级、标签、指派、估时）
- [ ] 评论、附件、活动流、时间记录
- [ ] 批量更新、保存筛选器、排序与分组

4. 规划域
- [ ] Cycles（冲刺）与进度计算
- [ ] Modules
- [ ] Milestones / Epics（开源范围内按最小可用实现）
- [ ] 依赖关系（blocked-by / blocking）

5. 视图域
- [ ] List View 查询与投影
- [ ] Kanban View（列规则 + WIP 校验）
- [ ] Calendar View
- [ ] 基础 Timeline 数据接口

6. 知识库域
- [ ] Pages/Wiki（层级、模板、修订）
- [ ] 页面评论与关联工作项

7. Intake 与通知域
- [ ] Intake（手动录入 + 表单导入）
- [ ] Inbox/通知中心（站内通知）
- [ ] Notification 路由策略（用户、角色、项目）

8. 搜索与分析域
- [ ] 全文检索（标题/描述/评论/页面）
- [ ] 基础指标（吞吐、周期、完成率、逾期）
- [ ] Dashboard 基础部件

9. Webhook 与集成域
- [ ] 事件总线（领域事件 -> outbox）
- [ ] Webhook 订阅、签名、重试、死信
- [ ] 导入导出（CSV first，其他源适配留扩展点）

10. MCP 域（AI 调用）
- [ ] MCP 传输层（stdio + http 可切换）
- [ ] 工具注册与 schema 管理
- [ ] 初始工具集：
  - [ ] `projects.list/create/get`
  - [ ] `work_items.list/create/update/get`
  - [ ] `work_items.add_comment`
  - [ ] `cycles.list/create/assign`
  - [ ] `pages.search/get/create`
  - [ ] `search.global`
- [ ] MCP 鉴权（token -> actor -> ACL）
- [ ] MCP 审计日志（输入摘要、输出状态、耗时）

11. PostgreSQL 缓存/队列/定时任务落地
- [ ] `cache_entries`：TTL、命名空间、主动失效
- [ ] `job_queue`：优先级、可见性超时、重试退避、死信
- [ ] `scheduled_jobs`：cron 解析、抢占执行、幂等键
- [ ] Worker 执行器（并发上限、优雅停机、指标）

12. 质量与安全
- [ ] 输入校验（长度/范围/枚举/语义）
- [ ] 敏感信息脱敏日志
- [ ] 速率限制与反滥用
- [ ] 关键路径单元测试 + 集成测试 + 契约测试
- [ ] CI：fmt/check/clippy/test + 漏洞扫描

13. 发布与运维
- [ ] DB migration 流程与回滚演练
- [ ] 版本策略（语义化版本 + API 兼容策略）
- [ ] 运行手册（故障处理、备份恢复）

14. 前端应用域（Web）
- [ ] 前端工程初始化（Monorepo 子应用、环境配置、构建脚本、Docker 镜像）
- [ ] 设计系统（颜色/字体/间距/token）、基础组件库（Button/Input/Modal/Table）
- [ ] 认证与会话（登录、退出、token 刷新、鉴权路由守卫）
- [ ] 项目与工作项核心页面：
  - [ ] 项目列表、项目概览
  - [ ] Work Item 列表（筛选、排序、分组、批量操作）
  - [ ] Work Item 详情（评论、活动流、附件、时间记录）
- [ ] 多视图页面：
  - [ ] Kanban
  - [ ] Calendar
  - [ ] 基础 Timeline
- [ ] 规划页面（Cycles、Modules、Milestones/Epics 最小可用）
- [ ] Wiki/Pages 编辑与浏览（富文本、目录、版本历史入口）
- [ ] 全局搜索与命令面板（快捷入口、最近访问）
- [ ] 通知中心与 Inbox 页面
- [ ] API SDK（OpenAPI/手写 Client）与错误提示统一处理
- [ ] 状态管理与缓存策略（分页、增量加载、乐观更新、失效重拉）
- [ ] 可观测性（前端日志、错误上报、关键交互埋点）
- [ ] E2E 测试（关键用户路径）与视觉回归（可选）

## 已完成
- [x] 明确技术栈与范围边界（开源能力 + MCP，排除商业功能）。
- [x] 形成覆盖业务域与平台域的全量任务清单（可作为执行基线）。

## 未完成
- [ ] 按阶段拆解为 Sprint Backlog（含人天估算、依赖、里程碑日期）。
- [ ] 输出首期 MVP（项目/工作项/评论 + MCP 核心工具）详细 API 与表结构。
- [ ] 建立仓库骨架与首批 migrations。
- [ ] 明确前端技术选型（框架、路由、状态管理、UI 方案）并输出页面信息架构。

## 对应 done/ 文档索引
- `log/done/chore_2026-02-13_create_plane_full_task_checklist.md`
- `log/done/chore_2026-02-13_add_frontend_scope_to_full_task_plan.md`
