# fix: Phase 5 Admin 管理面板 + 设置页面开发

## 背景
- 项目在 Phase 4 后仍缺失管理员管理面板、个人设置、工作区设置、成员管理与 Webhook 管理等关键页面。
- 需要接入新的成员/Webhook/搜索/导入导出 API，并升级全局导航。

## 变更点
- API 层
  - 新增 `src/lib/api/members.ts`：成员列表、邀请、移除。
  - 新增 `src/lib/api/webhooks.ts`：Webhook 列表、详情、创建、更新、删除。
  - 新增 `src/lib/api/search.ts`：全局搜索。
  - 新增 `src/lib/api/admin.ts`：管理员创建用户（复用 register）。
  - 新增 `src/lib/api/import-export.ts`：项目导出、工作区项目导入。
  - 更新 `src/lib/api/auth.ts`：新增 `me/register`，扩展 `User` 字段。
- 权限与导航
  - 新增 `src/lib/utils/auth.ts`：`isAdminUser` 权限判断。
  - 重构 `src/routes/(app)/+layout.svelte`：
    - 顶栏搜索入口与 `Ctrl/Cmd+K` 快捷键
    - 侧边栏新增搜索、个人设置
    - 工作区上下文新增成员/Webhook/设置入口
    - Admin 分组入口（仅管理员）
    - 页面挂载时调用 `/auth/me` 同步用户信息
  - 新增 `src/routes/(app)/admin/+layout.ts` 实现 admin 路由保护。
- 页面实现
  - 新增 `src/routes/(app)/admin/+page.svelte`：系统概览、快捷入口、系统状态。
  - 新增 `src/routes/(app)/admin/users/+page.svelte`：用户列表、创建、筛选、详情、禁用（占位态）。
  - 新增 `src/routes/(app)/admin/settings/+page.svelte`：系统配置占位页。
  - 新增 `src/routes/(app)/settings/+page.svelte`：个人资料、修改密码、通知偏好（`me` 接入 + 占位保存）。
  - 新增 `src/routes/(app)/workspace/[workspaceId]/settings/+page.svelte`：工作区基础信息、危险删除区。
  - 新增 `src/routes/(app)/workspace/[workspaceId]/members/+page.svelte`：成员列表、邀请、角色调整、移除确认。
  - 新增 `src/routes/(app)/workspace/[workspaceId]/webhooks/+page.svelte`：Webhook 列表、创建/编辑/删除。
  - 新增 `src/routes/(app)/search/+page.svelte`：分组搜索结果页。
  - 更新 `src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/+page.svelte`：项目导出下载、项目导入上传。

## 涉及文件
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

## 验证命令与结果
- 命令：`npm run check`
- 结果：通过（无 errors）。

## 后续建议
1. 后端新增全局用户列表/更新/禁用接口后，替换 admin 用户管理页中的占位逻辑。
2. 成员角色变更建议新增 PATCH 接口，避免当前“移除 + 重新邀请”带来的原子性风险。
3. 为搜索结果补充统一路由元数据（workspace/project/issue 映射字段），避免前端推断跳转。
