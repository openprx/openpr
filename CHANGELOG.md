# OpenPR Changelog

## 2026-02-13 - 阶段 1: 核心业务 API 完成 ✅

### 新增功能

#### 1. Issue 工作项 API（5 端点）
- `POST /api/v1/projects/:project_id/issues` - 创建工作项
- `GET /api/v1/projects/:project_id/issues` - 列出工作项（支持筛选）
- `GET /api/v1/issues/:id` - 获取详情
- `PUT /api/v1/issues/:id` - 更新工作项
- `DELETE /api/v1/issues/:id` - 删除工作项
- **功能**：状态管理（todo/in_progress/done）、优先级（low/medium/high/urgent）、负责人分配、截止日期

#### 2. 评论系统 API（4 端点）
- `POST /api/v1/issues/:issue_id/comments` - 创建评论
- `GET /api/v1/issues/:issue_id/comments` - 列出评论
- `PUT /api/v1/comments/:id` - 更新评论
- `DELETE /api/v1/comments/:id` - 删除评论
- **功能**：评论按时间排序、包含作者信息、权限控制

#### 3. 标签系统 API（7 端点）
- `POST /api/v1/workspaces/:workspace_id/labels` - 创建标签
- `GET /api/v1/workspaces/:workspace_id/labels` - 列出标签
- `PUT /api/v1/labels/:id` - 更新标签
- `DELETE /api/v1/labels/:id` - 删除标签
- `POST /api/v1/issues/:issue_id/labels/:label_id` - 添加标签到工作项
- `GET /api/v1/issues/:issue_id/labels` - 获取工作项标签
- `DELETE /api/v1/issues/:issue_id/labels/:label_id` - 移除标签
- **功能**：标签名唯一性、颜色、描述、多对多关联

#### 4. 活动流 API（3 端点）
- `GET /api/v1/workspaces/:workspace_id/activities` - 工作区活动流
- `GET /api/v1/projects/:project_id/activities` - 项目活动流
- `GET /api/v1/issues/:issue_id/activities` - 工作项活动流
- **功能**：审计日志、时间线、事件追踪、包含操作者信息

#### 5. 看板视图 API（1 端点）
- `GET /api/v1/projects/:project_id/board` - 获取看板视图
- **功能**：按状态分组、包含标签、负责人信息、拖拽式看板数据

#### 6. 迭代管理 API（4 端点）
- `POST /api/v1/projects/:project_id/sprints` - 创建迭代
- `GET /api/v1/projects/:project_id/sprints` - 列出迭代
- `PUT /api/v1/sprints/:id` - 更新迭代
- `DELETE /api/v1/sprints/:id` - 删除迭代
- **功能**：迭代名称唯一性、开始/结束日期、状态管理（planned/active/completed/cancelled）

### 数据库迁移
- **0003_labels.sql** - 标签表和工作项标签关联表
- **0004_sprints.sql** - 迭代表和 work_items.sprint_id 字段

### 新增文件
- **Entities (10 个)**: user, work_item, comment, label, work_item_label, activity, sprint
- **Routes (7 个)**: issue, comment, label, activity, board, sprint
- **Migrations (2 个)**: labels, sprints

### 代码统计
- **Rust 代码**: ~2500 行
- **API 端点**: 24 个
- **编译状态**: ✅ 通过（仅 unused warnings）

### 权限模型
- **工作项 CRUD**: 工作区成员可创建/更新，owner/admin 可删除
- **评论**: 作者可更新，作者或 owner/admin 可删除
- **标签**: 成员可创建/更新，owner/admin 可删除
- **迭代**: 成员可管理

---

## 下一步：阶段 2 - MCP Server 实现
- [ ] MCP 服务器骨架
- [ ] projects 工具
- [ ] issues 工具
- [ ] search 工具
- [ ] comments 工具
