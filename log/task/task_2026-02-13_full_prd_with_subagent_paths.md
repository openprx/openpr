# 任务：输出完整 PRD 并分配子代理规划路径

## 上下文/动机
- 需求：不只要一次性规划，而是要可持续推进的完整 PRD，并能并行分配不同规划路径。
- 目标：形成“主 PRD + 子代理工作流 + 分路径产物”，可直接用于后续分工执行。

## 结论与方案
- 采用“主文档 + 编排文档 + 5 条工作流文档”结构：
  - 主文档负责全局一致性（范围、需求、权限、数据、API、前端、NFR、测试、发布）。
  - 编排文档负责并行路径的 owner、节奏、合并门禁。
  - 各工作流文档负责分域深挖，持续迭代。
- 这样可以在后续迭代中持续补齐 PRD，而非一次性静态文档。

## 涉及文件
- `docs/prd/PRD_MASTER.md`
- `docs/prd/WORKSTREAM_ORCHESTRATION.md`
- `docs/prd/workstreams/WS1_product_domain.md`
- `docs/prd/workstreams/WS2_backend_api_mcp.md`
- `docs/prd/workstreams/WS3_data_platform.md`
- `docs/prd/workstreams/WS4_frontend_ux.md`
- `docs/prd/workstreams/WS5_qa_security_release.md`
- `log/task/task_2026-02-13_full_prd_with_subagent_paths.md`
- `log/done/chore_2026-02-13_create_full_prd_and_subagent_workstreams.md`
- `log/changelog.md`

## 计划
- [ ] 继续细化 WS1：补全所有模块 Given/When/Then
- [ ] 继续细化 WS2：输出 OpenAPI v1 草案与错误码全表
- [ ] 继续细化 WS3：输出 ERD 与索引覆盖说明
- [ ] 继续细化 WS4：输出页面线框与组件树
- [ ] 继续细化 WS5：输出发布前检查单与回滚演练脚本

## 已完成
- [x] 产出完整主 PRD（覆盖功能、权限、数据、API、前端、NFR、测试、发布）。
- [x] 产出子代理编排文档（并行规则、合并门禁、节奏）。
- [x] 产出 5 条分工路径文档（产品域、后端/API/MCP、数据平台、前端、QA/发布）。

## 未完成
- [ ] 将各 WS 文档从“纲要”扩展为“可编码级别契约”。
- [ ] 对齐每个里程碑的负责人、工时、开始/结束日期。
- [ ] 将 PRD 条目映射为 issue backlog（epic -> story -> task）。

## 对应 done/ 文档索引
- `log/done/chore_2026-02-13_create_full_prd_and_subagent_workstreams.md`
