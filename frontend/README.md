# OpenPR Frontend

OpenPR 前端应用，基于 SvelteKit + TypeScript + Tailwind CSS + shadcn-svelte 构建。

## 技术栈

- **框架：** SvelteKit 2.x (Svelte 5)
- **语言：** TypeScript
- **运行时：** Bun 1.3+
- **UI 库：** shadcn-svelte
- **样式：** Tailwind CSS v4
- **构建工具：** Vite 7

## 快速开始

### 安装依赖

```bash
bun install
```

### 开发服务器

```bash
bun run dev
```

访问 http://localhost:5173

### 构建生产版本

```bash
bun run build
```

### 预览生产版本

```bash
bun run preview
```

## 项目结构

```
src/
├── lib/
│   ├── api/              # API 客户端
│   │   ├── client.ts     # 基础 HTTP 客户端
│   │   ├── auth.ts       # 认证 API
│   │   ├── workspaces.ts # 工作区 API
│   │   ├── projects.ts   # 项目 API
│   │   ├── issues.ts     # 工作项 API
│   │   └── notifications.ts # 通知 API
│   ├── stores/           # Svelte 状态管理
│   │   ├── auth.ts       # 认证状态
│   │   └── toast.ts      # Toast 通知
│   └── components/       # 可复用组件
│       └── Toast.svelte  # Toast 通知组件
├── routes/               # 页面路由
│   ├── (auth)/           # 认证路由组
│   │   └── auth/login/   # 登录页
│   └── (app)/            # 应用路由组（需认证）
│       ├── inbox/        # 通知中心
│       └── workspace/    # 工作区
│           ├── [workspaceId]/
│           │   └── projects/
│           │       ├── +page.svelte         # 项目列表
│           │       └── [projectId]/
│           │           ├── +page.svelte     # 项目详情
│           │           ├── issues/          # 工作项列表/详情
│           │           ├── board/           # 看板视图
│           │           └── cycles/          # 迭代管理
│           └── +page.svelte                 # 工作区选择
└── app.css               # 全局样式

```

## 核心页面

### 已实现（8 个）

1. **登录页** - `/auth/login`
2. **工作区选择** - `/workspace`
3. **项目列表** - `/workspace/:workspaceId/projects`
4. **项目详情** - `/workspace/:workspaceId/projects/:projectId`
5. **工作项列表** - `/workspace/:workspaceId/projects/:projectId/issues`
6. **工作项详情** - `/workspace/:workspaceId/projects/:projectId/issues/:issueId`
7. **看板视图** - `/workspace/:workspaceId/projects/:projectId/board`
8. **Cycles 迭代** - `/workspace/:workspaceId/projects/:projectId/cycles`（占位页）
9. **通知中心** - `/inbox`

## 功能特性

### ✅ 已完成

- [x] API 客户端封装（统一错误处理、自动 Token 管理）
- [x] 认证流程（登录/登出/Token 刷新）
- [x] 路由守卫（AuthGuard）
- [x] 状态管理（Svelte stores）
- [x] Toast 通知系统
- [x] 响应式设计（桌面/平板/移动端）
- [x] Loading/Error/Empty 三态管理
- [x] 工作项 CRUD 操作
- [x] 评论功能
- [x] 看板视图
- [x] 通知中心

### 🚧 待完善

- [ ] 拖拽功能（看板）
- [ ] Cycles 迭代管理完整实现
- [ ] 图片上传
- [ ] Markdown 编辑器
- [ ] 实时通知（WebSocket）
- [ ] 搜索功能
- [ ] 无障碍访问优化（修复 a11y 警告）

## 环境变量

创建 `.env` 文件：

```bash
# API 基础 URL
VITE_API_BASE_URL=http://localhost:3000
```

## API 对接

后端 API 文档参考：`/opt/worker/code/openpr/docs/API_ENDPOINTS_PHASE3.md`

所有 API 请求通过 `$lib/api/client.ts` 中的 `apiClient` 发送，自动处理：

- JWT Token 注入（Authorization header）
- 统一错误响应格式
- Loading 状态管理
- LocalStorage Token 持久化

## 样式指南

### Tailwind CSS

- 移动端优先（mobile-first）
- 响应式断点：`sm` (640px), `md` (768px), `lg` (1024px), `xl` (1280px)
- 配色方案：Slate（主色）

### 组件规范

- 触摸友好（按钮最小 44x44px）
- 表格在移动端自动切换为卡片视图
- 侧边栏在小屏幕自动折叠

## 构建部署

### 生产构建

```bash
bun run build
# 输出：.svelte-kit/output/
```

### Adapter 配置

当前使用 `@sveltejs/adapter-auto`，支持自动检测部署平台：

- Node.js 服务器
- Vercel
- Netlify
- Cloudflare Pages
- 等等

如需指定 adapter，修改 `svelte.config.js`。

## 开发指南

### 添加新页面

1. 在 `src/routes/` 创建目录和 `+page.svelte`
2. 如需数据加载，添加 `+page.ts`
3. 如需服务端数据，添加 `+page.server.ts`

### 添加 API 方法

在 `src/lib/api/` 对应模块添加方法，使用 `apiClient` 发起请求。

### 添加全局状态

在 `src/lib/stores/` 创建新的 store 文件。

## 常见问题

### Q: 为什么使用 Bun 而不是 npm/pnpm？

A: Bun 速度更快，安装依赖和运行脚本比 npm 快 2-10 倍。

### Q: 如何处理 CORS 问题？

A: 在开发环境，Vite 的 proxy 功能可以代理 API 请求（见 `vite.config.ts`）。生产环境需后端配置 CORS。

### Q: 构建报错 a11y 警告？

A: 这些是无障碍访问警告，不影响构建。建议添加 `aria-label` 属性以提升用户体验。

## 许可证

双许可证： [MIT](../LICENSE-MIT) 或 [Apache-2.0](../LICENSE-APACHE)。
