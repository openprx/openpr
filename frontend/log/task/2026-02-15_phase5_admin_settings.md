# 任务：Phase 5 Admin 管理面板 + 设置页面开发

## context/motivation
- OpenPR 前端已完成 Phase 1-4（工作区、项目、Issue 看板、详情、Sprint、Inbox）。
- 本阶段需要补齐 Admin 管理与多层设置能力，覆盖管理员、个人、工作区维度。
- 同时需要接入成员管理、Webhook、全局搜索、导入导出，并完成导航升级与权限控制。

## conclusions/solution
- 新增 Phase 5 API 层：成员、Webhook、搜索、Admin（基于 register）、导入导出。
- 扩展认证 API：新增 `me/register`，并补充 `User` 角色/状态字段支持。
- 新增 Admin 页面群：仪表盘、用户管理、系统设置，并实现管理员权限限制。
- 新增设置页面群：个人设置、工作区设置、成员管理、Webhook 管理。
- 新增全局搜索页，支持 query 参数回填与分组展示。
- 在项目详情页增加导入/导出 JSON 功能。
- 更新应用布局导航：搜索入口、设置入口、Admin 入口、工作区成员/Webhook/设置入口，并支持 `Ctrl/Cmd+K`。

## involved files
- `src/lib/api/auth.ts`
- `src/lib/api/members.ts`
- `src/lib/api/webhooks.ts`
- `src/lib/api/search.ts`
- `src/lib/api/admin.ts`
- `src/lib/api/import-export.ts`
- `src/lib/utils/auth.ts`
- `src/routes/(app)/+layout.svelte`
- `src/routes/(app)/admin/+layout.ts`
- `src/routes/(app)/admin/+page.svelte`
- `src/routes/(app)/admin/users/+page.svelte`
- `src/routes/(app)/admin/settings/+page.svelte`
- `src/routes/(app)/settings/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/settings/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/members/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/webhooks/+page.svelte`
- `src/routes/(app)/search/+page.svelte`
- `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/+page.svelte`
- `log/done/fix_phase5_admin_settings_2026-02-15.md`
- `log/changelog.md`

## plan
1. 对齐现有 API 客户端与页面规范，新增 Phase 5 API 文件。
2. 扩展认证能力与管理员判断逻辑。
3. 实现 Admin、Settings、Workspace 管理相关新路由页面。
4. 升级应用侧边栏与顶部搜索入口，补齐权限控制。
5. 在项目详情接入导入/导出。
6. 执行 `npm run check` 并修复类型错误。
7. 完成 done 记录和 changelog 更新。

## 已完成
- 已新增 `members/webhooks/search/admin/import-export` API 文件。
- 已扩展 `authApi`，补充 `me/register` 与 `User` 扩展字段。
- 已新增 `isAdminUser` 权限工具，并在布局与 admin 路由中使用。
- 已重构 `src/routes/(app)/+layout.svelte`：新增搜索、设置、Admin、工作区管理导航及快捷键。
- 已新增 Admin 三页：`/admin`、`/admin/users`、`/admin/settings`。
- 已新增个人设置页：`/settings`。
- 已新增工作区设置页：`/workspace/[workspaceId]/settings`。
- 已新增工作区成员管理页：`/workspace/[workspaceId]/members`。
- 已新增 Webhook 管理页：`/workspace/[workspaceId]/webhooks`。
- 已新增全局搜索页：`/search`。
- 已在项目详情页接入项目导入/导出 JSON。
- 已新增本次 done 记录并更新 changelog。

## 未完成
- 未完成浏览器人工联调（需要后端运行态 + 真实数据验证所有路径）。
- 用户编辑/禁用、个人资料/密码、系统设置目前为占位或前端态，待后端扩展对应 API 后补全持久化。

## pending items
- 后端补充管理员用户列表、用户禁用、用户编辑接口后，将 `admin/users` 从前端态切换为真实 CRUD。
- 后端补充成员角色更新接口后，将当前“移除+重邀”改为单次 PATCH。
- 后端补充系统设置与个人资料更新接口后，替换占位保存逻辑。

## 对应 done/ 文档索引
- `log/done/fix_phase5_admin_settings_2026-02-15.md`
