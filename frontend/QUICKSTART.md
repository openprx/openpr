# OpenPR 前端快速开始

## 🚀 启动开发服务器

```bash
cd /opt/worker/code/openpr/frontend
bun run dev
```

访问：http://localhost:5173

## 📝 测试账号（需后端运行）

```
邮箱：demo@openpr.io
密码：password123
```

## 🎨 核心页面路由

| 页面 | 路由 | 说明 |
|------|------|------|
| 登录页 | `/auth/login` | 邮箱密码登录 |
| 工作台 | `/workspace` | 工作区选择 |
| 项目列表 | `/workspace/:id/projects` | 项目管理 |
| 项目详情 | `/workspace/:id/projects/:pid` | 项目概览 |
| Issue 列表 | `/workspace/:id/projects/:pid/issues` | 工作项管理 |
| Issue 详情 | `/workspace/:id/projects/:pid/issues/:iid` | 工作项详情 |
| 看板视图 | `/workspace/:id/projects/:pid/board` | Kanban 看板 |
| 迭代管理 | `/workspace/:id/projects/:pid/cycles` | Sprint 管理 |
| 通知中心 | `/inbox` | 消息通知 |

## 🔧 环境变量

创建 `.env` 文件：

```bash
VITE_API_BASE_URL=http://localhost:8081
```

## 📦 构建命令

```bash
# 开发
bun run dev

# 构建
bun run build

# 预览
bun run preview

# 类型检查
bun run check

# 代码检查
bunx eslint .

# 格式化
bunx prettier --write .
```

## 🐛 常见问题

### Q: 启动报错？

A: 确保已安装 Bun 1.3+：

```bash
curl -fsSL https://bun.sh/install | bash
```

### Q: API 请求失败？

A: 检查 `.env` 中的 `VITE_API_BASE_URL` 是否正确，确保后端服务已启动。

### Q: 登录后跳转到空白页？

A: 打开浏览器控制台，查看是否有 API 错误。确认后端数据库已迁移。

## 📖 开发文档

- [SvelteKit 文档](https://kit.svelte.dev/docs)
- [Tailwind CSS 文档](https://tailwindcss.com/docs)
- [shadcn-svelte 文档](https://www.shadcn-svelte.com/)

## 🎯 下一步

1. 启动后端 API 服务
2. 运行数据库迁移
3. 启动前端开发服务器
4. 访问 http://localhost:5173 并测试功能
