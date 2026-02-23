# fix: Phase 3 Issue 详情页 + 评论/标签/活动记录开发

## 背景
- 现有 Issue 详情页仅提供基础信息和新增评论，缺少编辑、标签、活动记录等核心协作能力。
- Phase 3 需要补齐详情页全流程交互，并保证 TypeScript 严格模式和移动端可用性。

## 变更点
- API 层
  - 新增 `src/lib/api/comments.ts`：`list/create/update/delete`。
  - 新增 `src/lib/api/labels.ts`：`list/create/addToIssue/removeFromIssue`。
  - 新增 `src/lib/api/activity.ts`：`list`。
  - 更新 `src/lib/api/issues.ts`：补充 `labels` 兼容字段，评论/活动类型补齐 `issue_id`，活动路径调整为 `/activity`。
- 通用组件
  - 新增 `src/lib/components/Textarea.svelte`。
  - 新增 `src/lib/components/Select.svelte`。
  - 新增 `src/lib/components/Tag.svelte`。
  - 更新 `src/lib/index.ts` 导出新组件。
- Markdown 渲染
  - 新增 `src/lib/utils/markdown.ts`，实现默认 HTML 转义 + 常见 Markdown 语法转换（标题、列表、粗体、斜体、代码、链接），用于 `{@html}` 安全输出。
- Issue 详情页
  - 重写 `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte`：
    - 面包屑 + 顶部操作（编辑/删除/关闭）
    - 标题与描述编辑
    - 状态/优先级/指派人下拉更新
    - 标签展示、添加、移除、创建
    - 评论列表与评论 CRUD（仅本人可编辑/删除）
    - 活动记录时间线
    - 移动端单列布局，评论输入区底部固定

## 涉及文件
- `src/lib/api/issues.ts`
- `src/lib/api/comments.ts`
- `src/lib/api/labels.ts`
- `src/lib/api/activity.ts`
- `src/lib/components/Textarea.svelte`
- `src/lib/components/Select.svelte`
- `src/lib/components/Tag.svelte`
- `src/lib/index.ts`
- `src/lib/utils/markdown.ts`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte`

## 验证命令与结果
- 命令：`npm run check`
- 结果：通过（0 errors，存在仓库既有 warnings，未新增阻断错误）。

## 后续建议
1. 与后端对齐 Issue 详情标签字段最终命名后，可移除 `labels/issue_labels` 双兼容逻辑。
2. 增加 Playwright/E2E 用例覆盖评论编辑删除和标签增删流程。
3. 后续统一处理当前仓库已有 a11y warnings，提升可访问性质量基线。
