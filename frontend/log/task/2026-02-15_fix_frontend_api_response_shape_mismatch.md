# 任务：批量修复前端 API 响应格式不匹配

## context/motivation
- 后端多个接口返回结构与前端假设不一致（数组、对象、嵌套对象混用）。
- 多个页面直接访问 `response.data`、`response.data.data`、`response.data.notifications`，在实际返回变化时会触发运行时崩溃。
- 目标是统一在 API 层与页面层增加防御性解包，保证空数据/异形数据时页面降级而不崩溃。

## conclusions/solution
- 新增 `src/lib/utils/api-helpers.ts`，提供 `extractList/extractItem/extractNumber` 三个通用安全提取函数。
- 批量改造 API 封装层（auth/workspaces/projects/issues/comments/labels/sprints/members/webhooks/notifications/search/activity），统一先解包再返回。
- 对关键页面补充防御性读取（layout、workspace projects、settings），消除高风险直读。
- 完成 `npm run check` 与 `cargo check`（使用 `CARGO_TARGET_DIR=/tmp/openpr-target` 规避当前目录权限限制）验证。

## involved files
- `src/lib/utils/api-helpers.ts`
- `src/lib/api/activity.ts`
- `src/lib/api/auth.ts`
- `src/lib/api/comments.ts`
- `src/lib/api/issues.ts`
- `src/lib/api/labels.ts`
- `src/lib/api/members.ts`
- `src/lib/api/notifications.ts`
- `src/lib/api/projects.ts`
- `src/lib/api/search.ts`
- `src/lib/api/sprints.ts`
- `src/lib/api/webhooks.ts`
- `src/lib/api/workspaces.ts`
- `src/routes/(app)/+layout.svelte`
- `src/routes/(app)/settings/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/+page.svelte`
- `log/done/fix_frontend_api_response_shape_mismatch_2026-02-15.md`
- `log/changelog.md`

## plan
1. 审核 `apiClient` 与全部受影响 API 封装。
2. 增加通用解包工具函数。
3. 统一改造 API 封装层返回格式。
4. 回到页面层补充剩余风险点。
5. 执行校验命令并修复报错。
6. 输出 done/task/changelog 记录。

## 已完成
- 已新增通用 API 防御提取工具。
- 已批量改造核心 API 封装，兼容数组/嵌套/对象三类响应。
- 已补充关键页面防御性读取。
- 已执行 `npm run check`，0 errors。
- 已执行 `CARGO_TARGET_DIR=/tmp/openpr-target cargo check`，通过。

## 未完成
- 未完成真实后端环境的逐页面手工联调（仅完成静态检查与编译检查）。
- 未为 API 异形响应增加自动化单元测试。

## pending items
- 为 `api-helpers` 增加单测，覆盖空对象、`data` 嵌套、字段缺失等场景。
- 在关键页面补充 e2e 用例，验证空列表与异常结构不崩溃。

## 对应 done/ 文档索引
- `log/done/fix_frontend_api_response_shape_mismatch_2026-02-15.md`
