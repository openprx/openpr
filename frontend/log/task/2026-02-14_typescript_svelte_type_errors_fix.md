# 任务：修复前端 TypeScript 与 Svelte 类型错误

## context/motivation
- `npm run check` 存在多处全局类型错误，阻塞静态检查通过。
- 主要问题集中在 API 请求头类型、Input 组件 props 定义、以及动态路由参数 `string | undefined` 传参。

## conclusions/solution
- `src/lib/api/client.ts` 改用 `Headers` API 处理请求头，避免对 `HeadersInit` 直接索引。
- `src/lib/components/Input.svelte` 增加严格 props：`oninput`、`helperText`、以及受限的 `autocomplete` 类型。
- 新增 `src/lib/utils/route-params.ts`，通过 `requireRouteParam` 统一将动态参数收敛为 `string`。
- 在所有报错动态路由页面接入 `requireRouteParam`，移除 `string | undefined` 传参错误。

## involved files
- `src/lib/api/client.ts`
- `src/lib/components/Input.svelte`
- `src/lib/utils/route-params.ts`
- `src/routes/(app)/workspace/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/board/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte`
- `log/done/fix_typescript_svelte_type_errors_2026-02-14.md`
- `log/changelog.md`

## plan
1. 运行 `npm run check` 获取完整错误清单。
2. 按错误文件逐一修复类型定义并保持原有逻辑不变。
3. 复跑 `npm run check`，确认错误清零。
4. 补充 task/done/changelog 记录。

## 已完成
- 已修复 API client 中 Authorization 头设置的类型错误。
- 已修复 Input 组件 `autocomplete`、`oninput`、`helperText` 的 props 类型问题。
- 已修复工作区页面对 Input 事件回调的类型推断问题。
- 已修复项目、看板、工作项详情等动态路由参数类型错误。
- 已新增路由参数断言工具并完成接入。
- 已执行 `npm run check`，结果为 `0 errors`（仍有既有 a11y warnings）。
- 已新增 done 文档并更新 changelog 索引。

## 未完成
- 暂无本任务范围内未完成项。

## pending items
- 若团队后续要求 `0 warnings`，需单独处理现有 Svelte a11y 警告。

## 对应 done/ 文档索引
- `log/done/fix_typescript_svelte_type_errors_2026-02-14.md`
