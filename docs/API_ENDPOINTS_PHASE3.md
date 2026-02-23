# OpenPR Phase 3 - 新增 API 端点

## Webhook 系统

### 创建 Webhook
```http
POST /api/v1/workspaces/:workspace_id/webhooks
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "My Webhook",
  "url": "https://example.com/webhook",
  "events": ["issue.created", "comment.created"],
  "active": true
}
```

### 列出 Webhooks
```http
GET /api/v1/workspaces/:workspace_id/webhooks
Authorization: Bearer <token>
```

### 获取 Webhook 详情
```http
GET /api/v1/workspaces/:workspace_id/webhooks/:webhook_id
Authorization: Bearer <token>
```

### 更新 Webhook
```http
PATCH /api/v1/workspaces/:workspace_id/webhooks/:webhook_id
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "Updated Name",
  "active": false
}
```

### 删除 Webhook
```http
DELETE /api/v1/workspaces/:workspace_id/webhooks/:webhook_id
Authorization: Bearer <token>
```

**支持的事件类型：**
- `issue.created`, `issue.updated`, `issue.deleted`, `issue.status_changed`
- `comment.created`, `comment.updated`, `comment.deleted`
- `project.created`, `project.updated`, `project.deleted`
- `sprint.created`, `sprint.updated`, `sprint.deleted`

---

## 通知中心

### 列出通知
```http
GET /api/v1/notifications?page=1&per_page=20&unread_only=true
Authorization: Bearer <token>
```

**查询参数：**
- `page` (可选): 页码，默认 1
- `per_page` (可选): 每页数量，默认 20，最大 100
- `unread_only` (可选): 仅返回未读通知，默认 false

**响应：**
```json
{
  "notifications": [...],
  "total": 100,
  "page": 1,
  "per_page": 20,
  "total_pages": 5,
  "unread_count": 15
}
```

### 标记通知为已读
```http
PATCH /api/v1/notifications/:id/read
Authorization: Bearer <token>
```

### 标记所有通知为已读
```http
PATCH /api/v1/notifications/read-all
Authorization: Bearer <token>
```

### 删除通知
```http
DELETE /api/v1/notifications/:id
Authorization: Bearer <token>
```

**通知类型：**
- `mention` - @提及
- `assignment` - 工作项分配
- `comment_reply` - 评论回复
- `issue_update` - 工作项更新
- `project_update` - 项目更新

---

## 全文搜索

### 搜索
```http
GET /api/v1/search?q=bug&type=issue&workspace_id=xxx&limit=50
Authorization: Bearer <token>
```

**查询参数：**
- `q` (必需): 搜索关键词
- `type` (可选): 搜索类型 - `issue`, `project`, `comment`（不指定则搜索所有）
- `workspace_id` (可选): 过滤工作区
- `project_id` (可选): 过滤项目
- `limit` (可选): 结果数量，默认 50，最大 100

**响应示例：**
```json
{
  "query": "bug",
  "total": 3,
  "results": [
    {
      "type": "issue",
      "id": "...",
      "key": "PROJ-123",
      "title": "Fix login bug",
      "description": "...",
      "status": "open",
      "project_id": "...",
      "rank": 0.45
    },
    {
      "type": "comment",
      "id": "...",
      "content": "This bug is critical",
      "work_item_id": "...",
      "created_by": "...",
      "created_at": "...",
      "rank": 0.32
    }
  ]
}
```

**搜索特性：**
- 基于 PostgreSQL 全文索引（tsvector + GIN）
- 权重配置：标题 > 描述 > 键
- 相关性排序（ts_rank）
- 权限过滤（仅返回用户有权限的资源）
- 前缀匹配支持

---

## 数据导入导出

### 导出项目
```http
GET /api/v1/export/project/:project_id?format=json
Authorization: Bearer <token>
```

**查询参数：**
- `format` (可选): `json` 或 `csv`，默认 `json`

**JSON 导出内容：**
- 项目元数据
- 所有工作项
- 所有评论
- 导出时间戳

**CSV 导出内容：**
- 仅工作项（Key, Title, Status, Priority, Type, Description, Created At, Updated At）

**响应头：**
```
Content-Type: application/json (or text/csv)
Content-Disposition: attachment; filename="project_XXX_export.json"
```

### 导入项目
```http
POST /api/v1/workspaces/:workspace_id/import/project
Authorization: Bearer <token>
Content-Type: application/json

{
  "project_key": "EXISTING",  // 可选：导入到现有项目
  "project_name": "New Project",  // 可选：创建新项目时必需
  "project_description": "Description",
  "issues": [
    {
      "key": "PROJ-1",  // 导入时会重新生成
      "title": "Issue title",
      "description": "Issue description",
      "status": "open",
      "priority": "high",
      "type": "bug"
    }
  ]
}
```

**响应：**
```json
{
  "project_id": "...",
  "project_key": "PROJ",
  "issues_created": 10,
  "issues_failed": 0,
  "errors": []
}
```

**导入特性：**
- 支持创建新项目或导入到现有项目
- 自动递增工作项编号
- 事务处理确保数据一致性
- 错误收集和报告
- 仅工作区管理员可导入

---

## 权限要求

| API | 权限要求 |
|-----|---------|
| Webhook 管理 | 工作区管理员 |
| 通知管理 | 当前用户（仅能管理自己的通知） |
| 搜索 | 工作区成员（仅搜索有权限的资源） |
| 导出项目 | 工作区成员 |
| 导入项目 | 工作区管理员 |

---

## 错误响应

所有 API 遵循统一的错误响应格式：

```json
{
  "error": "Error message"
}
```

**HTTP 状态码：**
- `400 Bad Request` - 请求参数错误
- `401 Unauthorized` - 未认证
- `403 Forbidden` - 无权限
- `404 Not Found` - 资源不存在
- `409 Conflict` - 资源冲突
- `500 Internal Server Error` - 服务器错误

---

## 使用示例

### 创建 Webhook 监听 Issue 创建
```bash
curl -X POST https://api.example.com/api/v1/workspaces/xxx/webhooks \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Slack Notifications",
    "url": "https://hooks.slack.com/services/XXX",
    "events": ["issue.created", "issue.updated"],
    "active": true
  }'
```

### 搜索所有工作项
```bash
curl -X GET "https://api.example.com/api/v1/search?q=authentication&type=issue" \
  -H "Authorization: Bearer $TOKEN"
```

### 导出项目为 JSON
```bash
curl -X GET "https://api.example.com/api/v1/export/project/xxx?format=json" \
  -H "Authorization: Bearer $TOKEN" \
  -o project_export.json
```

### 获取未读通知
```bash
curl -X GET "https://api.example.com/api/v1/notifications?unread_only=true" \
  -H "Authorization: Bearer $TOKEN"
```

---

## 数据库迁移

**运行迁移：**
```bash
# 按顺序执行
psql $DATABASE_URL < migrations/0005_webhooks.sql
psql $DATABASE_URL < migrations/0006_notifications.sql
psql $DATABASE_URL < migrations/0007_fulltext_search.sql
```

**检查全文索引：**
```sql
-- 查看搜索向量
SELECT key, title, search_vector FROM work_items LIMIT 1;

-- 测试搜索
SELECT key, title, ts_rank(search_vector, to_tsquery('english', 'bug:*')) as rank
FROM work_items
WHERE search_vector @@ to_tsquery('english', 'bug:*')
ORDER BY rank DESC
LIMIT 10;
```

---

## 下一步

1. 运行数据库迁移
2. 编译并启动 API 服务
3. 测试各个端点
4. 集成 Webhook 触发逻辑到现有 API
5. 集成通知生成逻辑
6. 实现 Worker 异步任务
