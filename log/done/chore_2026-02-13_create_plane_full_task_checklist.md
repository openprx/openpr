# chore: 创建 Plane Rust + MCP 全量开发任务清单

## 背景
- 需求：基于给定技术栈（Rust 2024 + Axum + SeaORM + PostgreSQL 一库多职能 + Docker）先完成架构分解，并输出覆盖完整业务逻辑的开发任务清单。
- 范围：仅开源能力与 MCP，对商业能力暂不纳入。

## 变更点
- 新增任务文档：`log/task/task_2026-02-13_plane-rust-mcp_full_delivery_plan.md`
- 任务文档已包含：
  - 上下文/动机
  - 结论与方案
  - 涉及文件
  - 计划（13 大域任务清单）
  - 已完成
  - 未完成
  - 对应 done/ 文档索引
- 计划内容覆盖：
  - 核心业务（项目、工作项、周期、模块、页面、通知、搜索、分析）
  - 平台能力（鉴权、审计、Webhook、导入导出）
  - MCP 能力（工具集、鉴权、审计）
  - PostgreSQL 缓存/队列/定时任务统一实现

## 涉及文件
- `log/task/task_2026-02-13_plane-rust-mcp_full_delivery_plan.md`
- `log/done/chore_2026-02-13_create_plane_full_task_checklist.md`
- `log/changelog.md`

## 验证命令与结果
- 命令：`ls -la log/task log/done`
- 结果：目标文档已创建。
- 命令：`sed -n '1,260p' log/task/task_2026-02-13_plane-rust-mcp_full_delivery_plan.md`
- 结果：文档包含必需字段与完整任务清单。

## 后续建议
1. 基于该总清单拆出 Phase-1（MVP）详细任务卡，附工时与负责人。
2. 先冻结 API 与表结构草案，再开始编码，避免接口反复重构。
3. 先做 MCP 最小工具集（读写 work item）验证 AI 调用闭环，再扩展更多工具。
