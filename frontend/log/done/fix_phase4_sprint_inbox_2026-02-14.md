# fix: Phase 4 Sprint 管理 + Inbox 收件箱开发

## 背景
- `cycles` 页面仍为占位实现，无法进行 Sprint 生命周期管理。
- `inbox` 页面基于旧通知协议，字段和接口与后端当前实现不一致。
- 项目需要完成 Phase 4 收尾，补齐 Sprint 与通知中心核心能力。

## 变更点
- API 层
  - 新增 `src/lib/api/sprints.ts`：`list/get/create/update/delete`。
  - 重写 `src/lib/api/notifications.ts`：对齐 `Notification` 新字段与 `list/markRead/delete`。
  - 更新 `src/lib/api/issues.ts`：增加可选 `sprint_id` 字段以支持 Sprint 统计与过滤。
- 通用组件
  - 新增 `src/lib/components/ProgressBar.svelte`（0-100 进度展示，可选标签与百分比）。
  - 更新 `src/lib/index.ts` 导出 `ProgressBar`。
- Sprint 管理页
  - 重写 `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/cycles/+page.svelte`：
    - 面包屑 + 创建 Sprint
    - 按状态分组（active/planned/completed）
    - 创建/编辑/删除 Sprint
    - planned -> active、active -> completed
    - 基于 Issue `sprint_id` 的完成率统计与进度条展示
    - 已完成 Sprint 折叠展示
    - 跳转 Sprint 看板（附带 `sprintId` 查询参数）
- Inbox 页面
  - 重写 `src/routes/(app)/inbox/+page.svelte`：
    - 未读/已读分组
    - 单条标记已读
    - 全部标记已读（循环调用单条接口）
    - 删除通知
    - 关联 Issue 跳转（通知 -> Issue -> Project -> 路由）
    - 加载中、空状态、错误重试
- 其他增强
  - `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/board/+page.svelte` 支持 `sprintId` 过滤。
  - `src/routes/(app)/+layout.svelte` 增加通知未读数量徽章。

## 涉及文件
- `src/lib/api/sprints.ts`
- `src/lib/api/notifications.ts`
- `src/lib/api/issues.ts`
- `src/lib/components/ProgressBar.svelte`
- `src/lib/index.ts`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/cycles/+page.svelte`
- `src/routes/(app)/inbox/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/board/+page.svelte`
- `src/routes/(app)/+layout.svelte`

## 验证命令与结果
- 命令：`npm run check`
- 结果：通过（无 errors）。

## 后续建议
1. 后端增加批量已读接口后，将 Inbox 的“全部已读”改为单请求调用。
2. Sprint 统计可下沉到后端聚合接口，减少前端计算与请求。
3. 增补 E2E 用例覆盖 Sprint 生命周期与通知跳转流程。
