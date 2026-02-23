# Frontend Requirements - OpenPR

## 技术栈
- **框架**: Svelte + SvelteKit
- **语言**: TypeScript
- **运行时**: Bun (替代 Node.js)
- **UI 库**: shadcn-svelte (推荐) 或 Skeleton
- **样式**: Tailwind CSS

## 响应式设计要求

### 断点（Breakpoints）
- **移动端**: < 768px
- **平板端**: 768px - 1023px
- **桌面端**: >= 1024px

### Tailwind 断点使用
```
sm:  640px  (小屏手机横屏)
md:  768px  (平板竖屏)
lg:  1024px (桌面/平板横屏)
xl:  1280px (大桌面)
2xl: 1536px (超大屏)
```

### 实现要点

#### 1. 移动端优先（Mobile First）
- 默认样式为移动端
- 使用 `md:` `lg:` 前缀向上扩展
- 示例：
  ```html
  <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3">
  ```

#### 2. 侧边栏（Sidebar）
- **桌面端 (lg+)**: 固定显示在左侧
- **移动端/平板**: 折叠为汉堡菜单，覆盖层显示
- 使用 `lg:block hidden` 控制可见性

#### 3. 表格/列表视图
- **桌面端**: 标准表格（`<table>`）
- **移动端**: 卡片视图（`<div>` 堆叠）
- 示例：
  ```html
  <!-- 桌面表格 -->
  <table class="hidden lg:table">...</table>
  
  <!-- 移动卡片 -->
  <div class="lg:hidden space-y-2">
    <div class="card">...</div>
  </div>
  ```

#### 4. 触摸友好交互
- **按钮最小尺寸**: 44px × 44px (Apple HIG 标准)
- **触摸目标间距**: >= 8px
- **示例**:
  ```html
  <button class="min-h-[44px] min-w-[44px] p-3">
    <!-- icon -->
  </button>
  ```

#### 5. 导航栏（Navbar）
- **桌面端**: 水平导航
- **移动端**: 汉堡菜单或底部导航栏
- 考虑使用 `<nav>` + `lg:flex` + `flex-col lg:flex-row`

#### 6. 内容布局
- **容器最大宽度**: `max-w-7xl` (1280px)
- **内边距**: `px-4 sm:px-6 lg:px-8`
- **网格**: `grid-cols-1 md:grid-cols-2 lg:grid-cols-3`

#### 7. 字体大小
- 移动端基础: `text-sm` (14px)
- 桌面端基础: `md:text-base` (16px)
- 标题自适应: `text-xl md:text-2xl lg:text-3xl`

#### 8. 图片/媒体
- 使用 `object-cover` 和 `aspect-ratio`
- 响应式尺寸: `w-full lg:w-1/2`

## 核心页面列表（8 个）

1. **登录/注册页** (`/auth`)
2. **工作区选择页** (`/workspaces`)
3. **项目列表页** (`/workspace/:id/projects`)
4. **看板视图页** (`/project/:id/board`)
5. **工作项列表页** (`/project/:id/issues`)
6. **工作项详情页** (`/issue/:id`)
7. **迭代管理页** (`/project/:id/sprints`)
8. **设置页** (`/settings`)

## Codex 实现检查清单

### 项目初始化
- [ ] `bun create svelte@latest` 初始化
- [ ] 安装 TypeScript 依赖
- [ ] 配置 Tailwind CSS
- [ ] 安装 shadcn-svelte
- [ ] 配置 API 客户端（基于 fetch/axios）

### 响应式实现
- [ ] 所有页面支持 3 种断点
- [ ] 移动端测试通过（Chrome DevTools）
- [ ] 触摸目标符合 44px 标准
- [ ] 表格在移动端切换为卡片
- [ ] 侧边栏折叠功能正常

### 组件库
- [ ] Button 组件（最小 44px）
- [ ] Card 组件
- [ ] Table/List 响应式组件
- [ ] Sidebar/Navbar 组件
- [ ] Modal/Dialog 组件
- [ ] Form 组件（带验证）

### 性能优化
- [ ] 路由懒加载
- [ ] 图片优化（WebP/AVIF）
- [ ] CSS 按需加载
- [ ] Lighthouse 移动端评分 > 90

## API 集成

### 认证
- Token 存储：`localStorage` 或 `sessionStorage`
- 自动刷新机制
- 401 自动跳转登录

### 数据获取
- SvelteKit `load` 函数
- 服务端渲染（SSR）支持
- 错误处理和加载状态

### 状态管理
- Svelte Stores 或 Pinia (如需要)
- 全局用户状态
- 工作区/项目上下文

## 部署要求
- 构建产物优化（`bun build`）
- 静态资源 CDN 支持
- 环境变量配置（`.env`）
- Docker 容器化（可选）
