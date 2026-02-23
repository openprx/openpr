# OpenPR - Open Source Project Management

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Docker](https://img.shields.io/badge/Docker-ready-blue.svg)](https://www.docker.com/)

OpenPR is a modern, open-source project management platform built with Rust, PostgreSQL, and SvelteKit. It provides issue tracking, project management, and MCP (Model Context Protocol) integration for AI-powered workflows.

## ✨ Features

- 🔐 **JWT-based Authentication** - Secure user registration and login
- 📊 **Project Management** - Workspaces, projects, and issue tracking
- 🏷️ **Labels & Sprints** - Organize issues with labels and sprint planning
- 🔔 **Notifications** - Real-time updates on project activities
- 🪝 **Webhooks** - Integrate with external services
- 🔍 **Full-text Search** - Fast search across issues and comments
- 🤖 **MCP Server** - AI integration via Model Context Protocol
- 🐳 **Docker Ready** - One-command deployment

## 🚀 Quick Start

### Prerequisites

- Docker & Docker Compose
- Git

### One-Command Deployment

```bash
# Clone the repository
git clone https://github.com/yourusername/openpr.git
cd openpr

# Copy environment file
cp .env.example .env

# Start all services
docker-compose up -d

# Verify deployment
curl http://localhost:8080/health  # API
curl http://localhost:8090/health  # MCP Server
curl http://localhost:3000         # Frontend
```

That's it! 🎉

- **Frontend**: http://localhost:3000
- **API**: http://localhost:8080
- **MCP Server**: http://localhost:8090
- **PostgreSQL**: localhost:5432

## 📚 Documentation

- [Deployment Guide](./DEPLOYMENT.md) - Detailed deployment instructions
- [API Documentation](./API_DOCUMENTATION.md) - API endpoints and examples
- [MCP Server Guide](./apps/mcp-server/README.md) - MCP integration
- [Frontend Guide](./frontend/README.md) - Frontend development

## 🏗️ Architecture

```
┌─────────────┐     ┌─────────────┐     ┌──────────────┐
│   Frontend  │────▶│     API     │────▶│  PostgreSQL  │
│  (Svelte)   │     │   (Rust)    │     │              │
└─────────────┘     └─────────────┘     └──────────────┘
                           │
                           ▼
                    ┌─────────────┐
                    │ MCP Server  │
                    │   (Rust)    │
                    └─────────────┘
```

## 🛠️ Tech Stack

### Backend
- **Language**: Rust 1.83+
- **Framework**: Axum (async web framework)
- **Database**: PostgreSQL 16
- **Authentication**: JWT
- **Serialization**: serde_json

### Frontend
- **Framework**: SvelteKit 2
- **Language**: TypeScript
- **Runtime**: Bun
- **Styling**: TailwindCSS
- **UI Components**: shadcn-svelte

### Infrastructure
- **Containerization**: Docker & Docker Compose
- **Web Server**: Nginx (for frontend)

## 📁 Project Structure

```
openpr/
├── apps/
│   ├── api/              # REST API server
│   ├── mcp-server/       # MCP protocol server
│   └── worker/           # Background job worker
├── crates/
│   ├── openpr-core/      # Core domain models
│   ├── openpr-db/        # Database layer
│   └── openpr-mcp/       # MCP protocol implementation
├── frontend/             # SvelteKit frontend
├── migrations/           # Database migrations
├── scripts/              # Deployment & test scripts
├── docker-compose.yml    # Docker orchestration
└── Dockerfile            # Multi-stage Docker build
```

## 🧪 Testing

```bash
# Run integration tests
bash scripts/test-api.sh

# Run MCP tests
bash scripts/test-mcp.sh

# Run full end-to-end tests
bash scripts/e2e-test.sh
```

## 🔧 Development

### Local Development (without Docker)

```bash
# 1. Start PostgreSQL
docker run -d -p 5432:5432 \
  -e POSTGRES_DB=openpr \
  -e POSTGRES_USER=openpr \
  -e POSTGRES_PASSWORD=openpr \
  postgres:16

# 2. Run migrations
bash scripts/init-db.sh

# 3. Start API server
cargo run -p api

# 4. Start MCP server
cargo run -p mcp-server -- --transport http --bind-addr 127.0.0.1:8090

# 5. Start frontend
cd frontend
bun install
bun run dev
```

## 🤝 Contributing

Contributions are welcome! Please read our [Contributing Guidelines](./CONTRIBUTING.md) first.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](./LICENSE) file for details.

## 🙏 Acknowledgments

- Built with [Axum](https://github.com/tokio-rs/axum)
- Frontend powered by [SvelteKit](https://kit.svelte.dev/)
- MCP Protocol by [Anthropic](https://modelcontextprotocol.io/)

## 📞 Support

- 🐛 **Bug Reports**: [GitHub Issues](https://github.com/yourusername/openpr/issues)
- 💬 **Discussions**: [GitHub Discussions](https://github.com/yourusername/openpr/discussions)
- 📧 **Email**: support@openpr.dev

---

Made with ❤️ by the OpenPR Team
