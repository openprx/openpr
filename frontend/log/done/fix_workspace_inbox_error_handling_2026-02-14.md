# fix: 修复工作区创建重复校验与通知中心数据库错误降级

## 背景
- 工作区创建成功后仍可能触发“请输入工作区名称”提示，用户感知为创建失败或前端异常。
- 访问通知中心时，后端返回 `database error`，前端缺乏明确的页面级错误提示。

## 变更点
- 在 `handleCreate()` 开头增加 `creating` 防重入判断，避免重复提交/重复校验。
- 工作区创建成功后，立即重置 `createForm`，随后关闭创建弹窗并刷新工作区列表。
- 创建按钮从 `type="submit"` 调整为 `type="button"`，避免和表单 `onsubmit` 路径重复触发。
- 通知中心新增 `errorMessage` 状态。
- 通知 API 返回错误时：
  - 清空通知列表与未读数，防止脏数据残留。
  - 将 `database error` 映射为“通知功能暂未启用”；其他错误映射为“无法加载通知，请稍后重试”。
  - 渲染错误提示卡片，保证页面可用。

## 涉及文件
- `src/routes/(app)/workspace/+page.svelte`
- `src/routes/(app)/inbox/+page.svelte`

## 验证命令与结果
- 命令：`npm run check`
- 结果：失败。
- 说明：失败由仓库既有问题导致（如 `src/lib/api/client.ts`、`src/lib/components/Input.svelte` 及多处路由参数类型错误）；本次修改文件未引入新的构建阻断类型。

## 后续建议
1. 独立修复全局 TypeScript/Svelte 诊断问题，恢复 `npm run check` 绿灯。
2. 后端通知模块补齐数据库表与接口实现后，可将“暂未启用”文案改为更精确错误码映射。
3. 为工作区创建流程增加 E2E 用例，覆盖“点击创建一次仅提交一次”的回归场景。
