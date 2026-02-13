# chore: 创建完整 PRD 与子代理规划路径文档包

## 背景
- 目标是把现有规划升级为“完整 PRD”，并支持持续迭代的并行规划路径，而非一次性交付。

## 变更点
- 新增主 PRD 文档：`docs/prd/PRD_MASTER.md`
  - 覆盖范围：功能需求、角色权限、数据模型、API 契约、前端要求、MCP、NFR、测试、发布。
- 新增子代理编排文档：`docs/prd/WORKSTREAM_ORCHESTRATION.md`
  - 定义 5 条并行路径、owner、门禁、节奏。
- 新增 5 条工作流文档：
  - `docs/prd/workstreams/WS1_product_domain.md`
  - `docs/prd/workstreams/WS2_backend_api_mcp.md`
  - `docs/prd/workstreams/WS3_data_platform.md`
  - `docs/prd/workstreams/WS4_frontend_ux.md`
  - `docs/prd/workstreams/WS5_qa_security_release.md`
- 新增任务文档：`log/task/task_2026-02-13_full_prd_with_subagent_paths.md`

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

## 验证命令与结果
- 命令：`find docs/prd -maxdepth 3 -type f | sort`
- 结果：主 PRD、编排文档、5 个 WS 文档均已存在。
- 命令：`sed -n '1,260p' docs/prd/PRD_MASTER.md`
- 结果：主 PRD 已覆盖完整开发维度。
- 命令：`sed -n '1,160p' docs/prd/WORKSTREAM_ORCHESTRATION.md`
- 结果：并行路径与合并门禁规则完整。

## 后续建议
1. 下一步先细化 WS2（OpenAPI）和 WS3（ERD），形成后端可编码契约。
2. 并行细化 WS4 页面线框，确保前后端合同一致。
3. 每周更新 `PRD_MASTER.md` 版本号与 `log/changelog.md` 索引。
