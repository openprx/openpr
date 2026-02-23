# fix: 修复前端 TypeScript 与 Svelte 类型错误

## 背景
- `npm run check` 报告 15 个类型错误，涉及 API 客户端、Input 组件 props、以及多处动态路由参数类型。
- 目标是在不改变业务逻辑的前提下使类型检查通过。

## 变更点
- `src/lib/api/client.ts`
  - 使用 `new Headers(options.headers)` + `headers.set(...)` 管理请求头。
  - 替代对 `HeadersInit` 的索引写法，消除 `Authorization` 属性索引错误。
- `src/lib/components/Input.svelte`
  - 引入 `HTMLInputAttributes` 严格约束输入属性类型。
  - 新增 `helperText` 与 `oninput` props 类型，并将 `autocomplete` 收窄为输入元素合法值。
  - 保留原有 `hint` 渲染能力，同时兼容 `helperText`。
- `src/routes/(app)/workspace/+page.svelte`
  - 将输入回调提取为显式类型的事件处理函数，消除隐式 `any`。
- `src/lib/utils/route-params.ts`
  - 新增 `requireRouteParam(value, name)`，将动态路由参数统一断言为 `string`。
- 动态路由页面接入 `requireRouteParam`
  - `src/routes/(app)/workspace/[workspaceId]/projects/+page.svelte`
  - `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/+page.svelte`
  - `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/board/+page.svelte`
  - `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/+page.svelte`
  - `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte`

## 涉及文件
- `src/lib/api/client.ts`
- `src/lib/components/Input.svelte`
- `src/lib/utils/route-params.ts`
- `src/routes/(app)/workspace/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/board/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte`

## 验证命令与结果
- 命令：`npm run check`
- 结果：通过（`svelte-check found 0 errors and 14 warnings in 5 files`）。
- 说明：剩余 14 项为既有 a11y warnings，不属于本次“类型错误修复”范围。

## 后续建议
1. 如需 CI 零告警，可单独开任务修复 `Modal`、`Card`、`Toast` 及 issue 详情页中的 a11y warnings。
2. 后续新增动态路由页面时统一使用 `requireRouteParam`，避免重复出现 `string | undefined` 传参问题。
