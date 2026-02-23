# fix: Phase 2 Issue 看板拖拽与列表视图实现

## 背景
- Phase 2 目标要求在项目内提供 Issue 看板能力，包括跨列拖拽更新状态、Issue 创建、筛选搜索与响应式布局。
- 现有实现仅有基础展示，缺少拖拽乐观更新、完整创建流程和备用列表视图能力。

## 变更点
- API 层
  - 在 `src/lib/api/client.ts` 增加 `put()`。
  - 重构 `src/lib/api/issues.ts` 为 `list/get/create/update/delete` 核心 CRUD（`update` 使用 `PUT`）。
  - 增加列表返回兼容处理（数组/分页结构）。
- 通用组件
  - 新增 `src/lib/components/Badge.svelte`（状态/优先级标签）。
  - 新增 `src/lib/components/Avatar.svelte`（指派人头像/占位）。
  - 更新 `src/lib/index.ts` 导出新组件。
- 看板页
  - 重写 `board/+page.svelte`，实现：
    - 4 列看板（Backlog / To Do / In Progress / Done）
    - HTML5 Drag & Drop（拖拽半透明、放置高亮）
    - 拖拽状态乐观更新，API 失败回滚
    - 快速创建 + 详细创建（Modal）
    - 搜索与筛选（优先级/指派人）
    - 点击卡片进入 Issue 详情页
    - 响应式列布局（移动 1 列，平板 2 列，桌面 4 列）
- 备用列表页
  - 重写 `issues/+page.svelte`，实现：
    - 表格视图（标题、状态、优先级、指派人、更新时间）
    - 移动端卡片视图
    - 搜索、筛选、排序（状态/优先级/指派人/时间/标题）
- 项目首页
  - 更新 `projects/[projectId]/+page.svelte` 对新 `issuesApi.list()` 返回结构的适配。

## 涉及文件
- `src/lib/api/client.ts`
- `src/lib/api/issues.ts`
- `src/lib/components/Badge.svelte`
- `src/lib/components/Avatar.svelte`
- `src/lib/index.ts`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/board/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/+page.svelte`

## 验证命令与结果
- 命令：`npm run check`
- 结果：通过（0 errors，存在既有 a11y warnings）。

## 后续建议
1. Phase 3 开发 Issue 详情与评论时，补齐指派人用户信息映射（从 `assignee_id` 到真实用户资料）。
2. 若后端补充 assignee 列表接口，可将“指派人 ID 输入”升级为可搜索下拉选择。
3. 统一清理仓库既有 a11y warnings，提升可访问性与 CI 稳定性。
