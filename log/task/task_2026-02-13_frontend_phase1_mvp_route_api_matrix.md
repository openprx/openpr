# 任务：前端 Phase-1（MVP）页面清单 + 路由 + API 对接矩阵

## 上下文/动机
- 当前已有全量任务清单，但需要可立即执行的前端 MVP 计划。
- 目标是尽快形成“可登录 -> 可看项目 -> 可管理工作项 -> 可看板拖拽”的最小业务闭环。

## 结论与方案
- 前端 MVP 优先交付 8 个页面（认证、项目、工作项列表、工作项详情、Kanban、Cycles、通知、个人工作台）。
- 路由采用分层结构：`/auth/*`、`/workspace/:workspaceId/*`。
- API 采用 BFF 直连后端 REST，统一错误 envelope 与分页协议。

## 涉及文件
- `log/task/task_2026-02-13_frontend_phase1_mvp_route_api_matrix.md`
- `log/done/chore_2026-02-13_create_frontend_phase1_mvp_outline.md`
- `log/changelog.md`

## 计划
1. 页面清单（MVP）
- [ ] 登录页：`/auth/login`
- [ ] 工作区项目列表：`/workspace/:workspaceId/projects`
- [ ] 项目概览：`/workspace/:workspaceId/projects/:projectId`
- [ ] Work Item 列表：`/workspace/:workspaceId/projects/:projectId/issues`
- [ ] Work Item 详情：`/workspace/:workspaceId/projects/:projectId/issues/:issueId`
- [ ] Kanban 视图：`/workspace/:workspaceId/projects/:projectId/board`
- [ ] Cycles 列表：`/workspace/:workspaceId/projects/:projectId/cycles`
- [ ] 通知中心：`/workspace/:workspaceId/inbox`
- [ ] 我的工作：`/workspace/:workspaceId/my-work`

2. 路由守卫与布局
- [ ] `AuthGuard`：未登录跳转 `/auth/login`
- [ ] `WorkspaceGuard`：校验成员关系与最小权限
- [ ] `AppLayout`：顶部导航 + 侧边导航 + 内容区

3. API 对接矩阵（首批）
- [ ] 认证
  - `POST /api/v1/auth/login` -> 登录
  - `POST /api/v1/auth/refresh` -> 刷新 token
  - `POST /api/v1/auth/logout` -> 退出
- [ ] 项目
  - `GET /api/v1/workspaces/{workspace_id}/projects`
  - `GET /api/v1/projects/{project_id}`
- [ ] 工作项
  - `GET /api/v1/projects/{project_id}/issues`
  - `POST /api/v1/projects/{project_id}/issues`
  - `GET /api/v1/issues/{issue_id}`
  - `PATCH /api/v1/issues/{issue_id}`
- [ ] 评论与活动
  - `GET /api/v1/issues/{issue_id}/comments`
  - `POST /api/v1/issues/{issue_id}/comments`
  - `GET /api/v1/issues/{issue_id}/activities`
- [ ] 看板与迭代
  - `GET /api/v1/projects/{project_id}/board`
  - `PATCH /api/v1/issues/{issue_id}/status`
  - `GET /api/v1/projects/{project_id}/cycles`
- [ ] 通知与个人
  - `GET /api/v1/inbox`
  - `PATCH /api/v1/inbox/{id}/read`
  - `GET /api/v1/my-work`

4. 页面到 API 映射（执行视角）
- [ ] 登录页 -> `auth/login|refresh|logout`
- [ ] 项目列表页 -> `workspaces/{id}/projects`
- [ ] 项目概览页 -> `projects/{id}` + 最近 issue 列表
- [ ] Issue 列表页 -> `projects/{id}/issues`（筛选/排序/分页）
- [ ] Issue 详情页 -> `issues/{id}` + `comments` + `activities`
- [ ] Kanban 页 -> `projects/{id}/board` + `issues/{id}/status`
- [ ] Cycles 页 -> `projects/{id}/cycles`
- [ ] Inbox 页 -> `inbox` + `inbox/{id}/read`
- [ ] My Work 页 -> `my-work`

5. MVP 交付标准
- [ ] 所有页面首屏 < 2s（本地开发数据量）
- [ ] 核心操作有 loading / empty / error 三态
- [ ] 关键交互（创建 issue、拖拽状态、评论）具备成功/失败反馈
- [ ] E2E 覆盖 3 条主路径：登录、创建并更新 issue、看板拖拽

## 已完成
- [x] 定义前端 MVP 页面边界与优先级。
- [x] 输出路由结构与鉴权守卫要求。
- [x] 输出页面-API 对接矩阵，可直接用于联调排期。

## 未完成
- [ ] 细化每个页面字段级 API contract（请求/响应 schema）。
- [ ] 产出组件拆分图（页面级/区块级/通用组件级）。
- [ ] 拆成两周 Sprint 任务卡（负责人、工时、依赖）。

## 对应 done/ 文档索引
- `log/done/chore_2026-02-13_create_frontend_phase1_mvp_outline.md`
