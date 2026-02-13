# chore: 在总任务清单中补充前端能力域

## 背景
- 反馈：现有任务大纲主要是后端与平台能力，前端建设内容不完整，无法支撑完整产品交付。

## 变更点
- 在总任务清单中新增“14. 前端应用域（Web）”。
- 前端范围包含：
  - 工程初始化与 Docker 化
  - 设计系统与基础组件
  - 认证与会话
  - 项目/工作项核心页面
  - 多视图（Kanban/Calendar/Timeline）
  - 规划页、Wiki 页、全局搜索、通知中心
  - API SDK、状态管理、可观测性、E2E
- 在“未完成”增加前端技术选型与信息架构输出项。
- 在任务文档“对应 done/ 文档索引”追加本记录链接。

## 涉及文件
- `log/task/task_2026-02-13_plane-rust-mcp_full_delivery_plan.md`
- `log/done/chore_2026-02-13_add_frontend_scope_to_full_task_plan.md`
- `log/changelog.md`

## 验证命令与结果
- 命令：`sed -n '1,320p' log/task/task_2026-02-13_plane-rust-mcp_full_delivery_plan.md`
- 结果：已出现“14. 前端应用域（Web）”与新增未完成项。
- 命令：`sed -n '1,120p' log/changelog.md`
- 结果：changelog 已新增本条索引。

## 后续建议
1. 单独产出前端 IA（信息架构）与路由清单，避免页面反复改版。
2. 优先交付 Work Item 列表 + 详情 + Kanban 三个页面，形成最小可用闭环。
3. 与后端同步冻结 MVP API，减少前后端联调返工。
