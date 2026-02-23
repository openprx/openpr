# Contributing to OpenPR

Thank you for your interest in contributing to OpenPR! 🎉

## Code of Conduct

Be respectful, inclusive, and constructive. We're all here to build great software together.

## Getting Started

1. **Fork the repository**
2. **Clone your fork**
   ```bash
   git clone https://github.com/yourusername/openpr.git
   cd openpr
   ```
3. **Set up development environment**
   ```bash
   bash scripts/start.sh
   ```

## Development Workflow

### 1. Create a Branch

```bash
git checkout -b feature/my-awesome-feature
```

Branch naming conventions:
- `feature/` - New features
- `fix/` - Bug fixes
- `docs/` - Documentation updates
- `refactor/` - Code refactoring
- `test/` - Test improvements

### 2. Make Your Changes

Follow our coding standards:
- **Rust**: See [RUST_CODING_STANDARD.md](./RUST_CODING_STANDARD.md)
- **Frontend**: ESLint + Prettier (run `bun run lint`)
- **Commits**: Use [Conventional Commits](https://www.conventionalcommits.org/)

Example commit messages:
```
feat: add user profile editing
fix: correct JWT token expiration
docs: update API documentation
test: add integration tests for auth flow
```

### 3. Test Your Changes

```bash
# Run API tests
bash scripts/test-api.sh

# Run MCP tests
bash scripts/test-mcp.sh

# Run E2E tests
bash scripts/e2e-test.sh

# Run Rust tests
cargo test

# Run frontend tests
cd frontend && bun run test
```

### 4. Format Your Code

```bash
# Rust
cargo fmt

# Frontend
cd frontend && bun run format
```

### 5. Submit a Pull Request

1. Push your branch to your fork
2. Open a PR against `main` branch
3. Fill out the PR template
4. Wait for review

## Pull Request Guidelines

- **Title**: Clear and descriptive
- **Description**: Explain what and why
- **Tests**: All tests must pass
- **Documentation**: Update docs if needed
- **Breaking changes**: Clearly marked and explained

## Project Structure

```
openpr/
├── apps/              # Applications (api, worker, mcp-server)
├── crates/            # Shared libraries
├── frontend/          # SvelteKit frontend
├── migrations/        # Database migrations
├── scripts/           # Deployment & utility scripts
└── docs/              # Documentation
```

## Development Tips

### Local Development

```bash
# Backend (without Docker)
cargo run -p api

# Frontend (hot reload)
cd frontend && bun run dev

# Database only
docker run -p 5432:5432 -e POSTGRES_PASSWORD=openpr postgres:16
```

### Debugging

```bash
# Rust with debug logs
RUST_LOG=debug cargo run -p api

# Check database
docker-compose exec postgres psql -U openpr -d openpr

# View API logs
docker-compose logs -f api
```

### Database Migrations

1. Create file: `migrations/NNNN_description.sql`
2. Write SQL (use `IF NOT EXISTS` for safety)
3. Test: `bash scripts/init-db.sh`
4. Commit with migration

## Testing Guidelines

- **Unit tests**: For business logic
- **Integration tests**: For API endpoints
- **E2E tests**: For full workflows

Write tests that:
- Are fast and isolated
- Have clear assertions
- Cover edge cases
- Use descriptive names

## Documentation

- **Code comments**: Explain *why*, not *what*
- **API docs**: Update `API_DOCUMENTATION.md`
- **Deployment**: Update `DEPLOYMENT.md`
- **README**: Keep it current

## Questions?

- 🐛 **Bugs**: [GitHub Issues](https://github.com/yourusername/openpr/issues)
- 💬 **Discussions**: [GitHub Discussions](https://github.com/yourusername/openpr/discussions)
- 📧 **Email**: dev@openpr.dev

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

---

Happy coding! 🚀
