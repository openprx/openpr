# 任务：Phase 3 Issue 详情页 + 评论/标签/活动记录开发

## context/motivation
- OpenPR 前端已完成 Phase 1（工作区/项目）和 Phase 2（Issue 看板/拖拽）。
- 本任务目标是补齐 Phase 3 的 Issue 详情协作能力，包括详情编辑、评论、标签和活动记录，且保持移动端可用。

## conclusions/solution
- 新增独立 API 层文件：评论、标签、活动记录。
- 重写 Issue 详情页为左右分栏（移动端单栏），支持：
  - 标题/描述编辑与保存
  - 状态/优先级/指派人下拉更新
  - 标签添加/移除与项目标签创建
  - 评论新增、编辑、删除（仅当前用户）
  - 活动记录时间线展示
- 新增通用组件 `Textarea`、`Select`、`Tag`，统一表单交互风格。
- 补充安全 Markdown 渲染工具（默认转义 HTML 后再进行 Markdown 语法转换），用于 Issue 描述与评论显示。

## involved files
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
- `log/done/fix_phase3_issue_detail_comments_activity_2026-02-14.md`
- `log/changelog.md`

## plan
1. 盘点现有 Issue API 和详情页实现，识别可复用和冲突点。
2. 拆分评论/标签/活动 API 文件并对齐后端路径。
3. 新增通用输入组件（Textarea/Select/Tag）。
4. 重写 Issue 详情页并接入评论、标签、活动记录能力。
5. 完成 Markdown 渲染与 XSS 风险控制。
6. 运行 `npm run check` 校验。
7. 记录 done 文档并更新 changelog。

## 已完成
- 已新增 `commentsApi`、`labelsApi`、`activityApi`。
- 已重写 Issue 详情页核心布局和交互逻辑。
- 已实现评论 CRUD（仅当前用户可编辑/删除）。
- 已实现标签添加/移除和项目标签创建入口。
- 已实现状态/优先级/指派人下拉更新。
- 已实现活动记录展示。
- 已实现移动端单列布局与底部固定评论输入区。
- 已执行 `npm run check`，结果为 0 errors（保留既有 warnings）。
- 已新增 done 文档并更新 changelog。

## 未完成
- 未执行容器构建与重启命令（依赖仓库根目录与本机容器环境）。
- 未进行联调账号的浏览器手工回归（需后端运行态支持）。

## pending items
- 与后端联调确认 Issue 详情返回中的标签字段最终结构（`labels` 或 `issue_labels`）。
- 可在下一阶段统一处理仓库既有 a11y warnings。

## 对应 done/ 文档索引
- `log/done/fix_phase3_issue_detail_comments_activity_2026-02-14.md`
