# chore: 产出前端 Phase-1 MVP 页面/路由/API 对接矩阵

## 背景
- 需要把“全量任务计划”收敛为可立即执行的前端 MVP 实施清单。

## 变更点
- 新增任务文档：`log/task/task_2026-02-13_frontend_phase1_mvp_route_api_matrix.md`
- 文档包含：
  - MVP 页面清单（9 个）
  - 路由与鉴权守卫
  - API 对接矩阵（认证、项目、工作项、评论活动、看板迭代、通知个人）
  - 页面到 API 的执行映射
  - MVP 验收标准（性能、状态反馈、E2E）

## 涉及文件
- `log/task/task_2026-02-13_frontend_phase1_mvp_route_api_matrix.md`
- `log/done/chore_2026-02-13_create_frontend_phase1_mvp_outline.md`
- `log/changelog.md`

## 验证命令与结果
- 命令：`sed -n '1,260p' log/task/task_2026-02-13_frontend_phase1_mvp_route_api_matrix.md`
- 结果：包含必需章节与页面/路由/API 矩阵。
- 命令：`sed -n '1,120p' log/changelog.md`
- 结果：包含本次变更索引。

## 后续建议
1. 与后端共同冻结 Phase-1 API schema（字段、错误码、分页协议）。
2. 先并行开发 `issues` 列表和详情，再接入 Kanban 拖拽。
3. E2E 先覆盖主链路，再补充边界场景。
