# 任务：修复工作区创建与通知中心错误

## context/motivation
- 工作区页面在创建成功后，仍出现“请输入工作区名称”提示，影响用户信任与操作连续性。
- 通知中心在后端返回 `{"error":"database error"}` 时未提供可理解的降级体验。

## conclusions/solution
- 工作区创建流程增加防重入，避免重复触发 `handleCreate()`；创建成功后先清空 `createForm`，再关闭弹窗并刷新列表。
- 通知中心新增页面级错误状态，当 API 失败时渲染友好提示（数据库异常场景提示“通知功能暂未启用”），同时保证页面不崩溃。

## involved files
- `src/routes/(app)/workspace/+page.svelte`
- `src/routes/(app)/inbox/+page.svelte`
- `log/done/fix_workspace_inbox_error_handling_2026-02-14.md`
- `log/changelog.md`

## plan
1. 检查工作区创建提交路径，消除重复触发并重置创建表单状态。
2. 为通知中心请求增加 error 状态与 UI 降级渲染。
3. 运行校验命令并记录结果。
4. 写入 `task/done/changelog` 追踪文档。

## 已完成
- 已在 `handleCreate()` 增加 `creating` 防重入保护。
- 已在工作区创建成功分支中重置 `createForm` 后关闭弹窗。
- 已将创建按钮改为 `type="button"`，避免提交路径重复触发。
- 已在通知中心增加 `errorMessage` 状态与数据库错误友好文案。
- 已在通知中心错误状态下渲染降级提示卡片，避免错误时页面失效。
- 已执行 `npm run check` 并记录结果。
- 已新增 done 记录与 changelog 索引。

## 未完成
- 暂无本任务范围内未完成项。

## pending items
- 当前仓库存在多处既有 TypeScript/Svelte 诊断错误，需后续独立任务清理。

## 对应 done/ 文档索引
- `log/done/fix_workspace_inbox_error_handling_2026-02-14.md`
