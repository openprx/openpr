# fix: 批量修复前端 API 响应格式不匹配

## 背景
- 后端接口在不同资源上存在数组/对象/嵌套对象等返回差异。
- 前端多个页面对响应结构假设过强，导致 `.map/.filter/.length` 在 `undefined` 上触发崩溃。

## 变更点
- 新增通用解包工具
  - `src/lib/utils/api-helpers.ts`
  - 提供：`extractList`、`extractItem`、`extractNumber`。
- 批量改造 API 封装层
  - `src/lib/api/auth.ts`：兼容 `/auth/me` 的 `{ user: ... }` 与直接对象。
  - `src/lib/api/workspaces.ts`：列表/详情/创建/更新/成员统一安全解包。
  - `src/lib/api/projects.ts`：`list` 兼容数组与分页对象；详情/创建/更新统一解包。
  - `src/lib/api/issues.ts`：列表/详情/创建/更新/评论/活动统一解包。
  - `src/lib/api/comments.ts`、`src/lib/api/labels.ts`、`src/lib/api/sprints.ts`、`src/lib/api/members.ts`、`src/lib/api/webhooks.ts`：统一列表与对象返回兼容。
  - `src/lib/api/notifications.ts`：兼容 `{notifications,...}` 与其他变体，补全分页与未读数兜底。
  - `src/lib/api/search.ts`、`src/lib/api/activity.ts`：兼容数组或嵌套列表。
- 页面层补充防御
  - `src/routes/(app)/+layout.svelte`：未读数读取增加安全提取。
  - `src/routes/(app)/workspace/[workspaceId]/projects/+page.svelte`：项目列表读取增加数组防御。
  - `src/routes/(app)/settings/+page.svelte`：个人信息读取改为 `extractItem`，去除 `any` 直读。

## 涉及文件
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

## 验证命令与结果
- 命令：`npm run check`
- 结果：通过（`svelte-check found 0 errors and 0 warnings`）。
- 命令：`CARGO_TARGET_DIR=/tmp/openpr-target cargo check`
- 结果：通过（编译成功，存在若干 warning，无 error）。

## 后续建议
1. 给 `src/lib/utils/api-helpers.ts` 增加单元测试，锁定兼容行为。
2. 对高风险页面（项目列表、Issue 详情、通知中心）补充 e2e 空响应场景。
3. 后端逐步统一响应规范后，可减少前端兜底分支并收敛类型定义。
