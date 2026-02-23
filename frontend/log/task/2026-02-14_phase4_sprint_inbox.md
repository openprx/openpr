# 任务：Phase 4 Sprint 管理 + Inbox 收件箱开发

## context/motivation
- OpenPR 前端已完成 Phase 1-3，当前需完成最后阶段：Sprint 管理和 Inbox 收件箱。
- 既有 `cycles` 页面与 `inbox` 页面为占位/旧协议实现，无法满足当前后端 API 与交互目标。

## conclusions/solution
- 新增 Sprint API 层，覆盖创建、列表、详情、更新、删除。
- 重构 Notification API 层，对齐后端新字段与接口（`is_read`、`related_issue_id`、`PUT /read`）。
- 新增 `ProgressBar` 通用组件并在 Sprint 管理页使用。
- 重写 Sprint 页面：状态分组、创建/编辑/删除、开始/完成、进度统计、跳转看板。
- 重写 Inbox 页面：未读/已读分组、单条/全部已读、删除、错误重试、空状态、关联 Issue 跳转。
- 增强看板页面：支持从 Sprint 页面携带 `sprintId` 进行过滤展示。
- 增强侧边栏：显示未读通知徽章。

## involved files
- `src/lib/api/sprints.ts`
- `src/lib/api/notifications.ts`
- `src/lib/api/issues.ts`
- `src/lib/components/ProgressBar.svelte`
- `src/lib/index.ts`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/cycles/+page.svelte`
- `src/routes/(app)/inbox/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/board/+page.svelte`
- `src/routes/(app)/+layout.svelte`
- `log/done/fix_phase4_sprint_inbox_2026-02-14.md`
- `log/changelog.md`

## plan
1. 对齐现有 API 客户端与页面编码风格，识别需改造点。
2. 新增/重构 Sprint 与 Notification API 文件。
3. 实现通用 `ProgressBar` 组件并导出。
4. 完整重写 Sprint 管理页面交互与状态分组。
5. 完整重写 Inbox 页面交互与错误处理。
6. 补充看板 Sprint 过滤与侧边栏未读徽章。
7. 执行 `npm run check` 校验类型与 Svelte 检查。
8. 更新 done 与 changelog 记录。

## 已完成
- 已新增 `src/lib/api/sprints.ts` 并接入 CRUD。
- 已重写 `src/lib/api/notifications.ts` 对齐新后端协议。
- 已新增 `src/lib/components/ProgressBar.svelte` 并在 `src/lib/index.ts` 导出。
- 已在 `src/lib/api/issues.ts` 添加可选 `sprint_id` 字段用于统计/过滤。
- 已重写 Sprint 管理页面（分组、创建/编辑/删除、开始/完成、进度条、看板跳转、移动端布局、错误重试）。
- 已重写 Inbox 页面（未读/已读分组、单条已读、全部已读、删除、空状态、错误重试、关联 Issue 跳转）。
- 已在看板页面增加 `sprintId` 过滤支持。
- 已在应用侧边栏增加未读通知徽章。

## 未完成
- 未执行容器重建与重启命令（依赖仓库根目录容器运行环境）。
- 未完成浏览器端人工联调回归（需后端运行态和测试账号登录）。

## pending items
- 如后端后续提供批量已读接口，可替换当前前端循环调用方案以减少请求数。
- 可进一步在看板中显示 Sprint 名称而非仅展示 `sprintId`。

## 对应 done/ 文档索引
- `log/done/fix_phase4_sprint_inbox_2026-02-14.md`
