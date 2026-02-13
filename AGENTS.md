# Repository Guidelines

## Project Structure & Module Organization
This repository is currently a minimal scaffold. As code is added, use this structure to keep contributions consistent:
- `src/`: application or library source code.
- `crates/*/src/`: workspace crates source code.
- `tests/`: automated tests mirroring source paths.
- `assets/`: static files (fixtures, images, sample data).
- `log/`: project records and delivery trace files.

For Rust-specific standards, follow `RUST_CODING_STANDARD.md`.

## Build, Test, and Development Commands
No build system is configured yet. For Rust projects, standardize around:
- `cargo fmt -- --check`: formatting check.
- `cargo check`: compile without producing binaries.
- `cargo clippy -- -D warnings`: lint with warnings treated as errors.
- `cargo test`: run unit/integration tests.

## Coding Style & Naming Conventions
Use `rustfmt` output as source of truth.
- Indentation: 4 spaces in Rust code, 2 spaces for YAML/JSON/Markdown.
- Naming: modules/files in `snake_case`, types/traits in `PascalCase`, functions in `snake_case`.
- Keep public APIs minimal and avoid exposing implementation details.

## Testing Guidelines
Add tests with every behavior change.
- Put unit tests close to modules or under `tests/` for integration coverage.
- Use descriptive names like `test_retries_on_timeout`.
- Cover edge cases and regression paths for bug fixes.

## Commit & Pull Request Guidelines
Use clear, imperative commit messages.
- Commit format: `<type>: <summary>` (e.g., `fix: handle missing config path`).
- Keep commits atomic and focused.
- PRs should include: purpose, key changes, test evidence, and linked issue/task.
- Add logs/screenshots when changing user-visible behavior.

## Update Records (Mandatory)
Use `log/changelog.md` as the index entry point; always check latest items there first.

Task analysis requirements:
- Create a task document in `log/task/`.
- Include at least: context/motivation, conclusions/solution, involved files, plan, completed items, pending items.
- Maintain a list of related fix documents (`log/done/`) inside the task document for traceability across multiple implementations.
- Every new `log/task/` document MUST contain these explicit sections:
  - `未完成`
  - `已完成`
  - `对应 done/ 文档索引` (list paths under `log/done/`)

Change implementation requirements:
- Any code/config/doc modification must create a record document in `log/done/` (recommended prefix: `fix_` or `chore_`).
- Each done record must include: background, change points, involved files, validation commands/results, and follow-up suggestions.
- Add a corresponding index entry in `log/changelog.md` pointing to that done record.

## Security & Configuration Tips
Do not commit secrets, tokens, or real credentials. Keep sensitive config in environment variables or untracked local files, and provide safe examples (e.g., `.env.example`) when new settings are introduced.
