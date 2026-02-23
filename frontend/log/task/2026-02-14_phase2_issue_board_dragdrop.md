# 任务：Phase 2 Issue 看板（拖拽功能）开发

## context/motivation
- OpenPR 前端需从 Phase 1（工作区/项目）进入 Phase 2，补齐 Issue 看板核心协作能力。
- 目标是交付可用的 Issue 看板、备用列表视图、Issue CRUD 前端接入和拖拽状态流转。

## conclusions/solution
- 重构 `src/lib/api/issues.ts`，实现 `list/get/create/update/delete` 五个核心方法，并将更新请求切换为后端要求的 `PUT /api/v1/issues/:id`。
- 在看板页实现 4 列布局、HTML5 拖拽、放置高亮、拖拽半透明、乐观更新与失败回滚。
- 新增快速创建与详细创建两种 Issue 创建入口，支持标题、描述、状态、优先级、指派人。
- 在备用列表页实现表格/移动端卡片双视图，支持搜索、筛选（状态/优先级/指派人）和排序。
- 补充通用组件 `Badge` 与 `Avatar`，复用优先级/状态标签与指派人展示。

## involved files
- `src/lib/api/client.ts`
- `src/lib/api/issues.ts`
- `src/lib/components/Badge.svelte`
- `src/lib/components/Avatar.svelte`
- `src/lib/index.ts`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/board/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/+page.svelte`
- `log/done/fix_phase2_issue_board_dragdrop_2026-02-14.md`
- `log/changelog.md`

## plan
1. 检查现有 Issue API、看板页和列表页实现，明确兼容字段与路由依赖。
2. 改造 API 层并补齐 `PUT` 请求能力。
3. 实现看板拖拽、创建、筛选与响应式布局。
4. 实现 Issue 列表备用视图（搜索/筛选/排序）。
5. 运行 `npm run check` 验证类型检查。
6. 写入 done 记录与 changelog 索引。

## 已完成
- 已完成 API 层重构与 `PUT` 更新接入。
- 已完成看板页拖拽、乐观更新、失败回滚和状态同步。
- 已完成快速创建和详细创建表单。
- 已完成看板搜索/优先级筛选/指派人筛选。
- 已完成 Issue 卡片信息展示（标题、优先级、指派人、编号）。
- 已完成备用列表页（表格 + 移动端卡片）及搜索筛选排序。
- 已执行 `npm run check`，结果为 0 errors（存在既有 a11y warnings）。
- 已新增 done 文档并更新 changelog。

## 未完成
- 未执行容器构建与重启命令（需在仓库根目录执行并依赖本机 Docker/Podman 环境）。

## pending items
- 可在 Phase 3 统一处理现有全局 a11y warning（非本次功能阻断项）。

## 对应 done/ 文档索引
- `log/done/fix_phase2_issue_board_dragdrop_2026-02-14.md`
