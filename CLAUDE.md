# CLAUDE.md — OpenPRX Rust Production Code Standards

This file is loaded by Claude Code on every session. These rules are MANDATORY.

## Rust Edition: 2024

## Build & Test
```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release --all-features
```

## Seven Iron Rules (Strictly Enforced)

1. **NO panic-capable unwrapping** — `.unwrap()`, `.expect()`, and any shorthand that can panic are BANNED in production code. Use `?`, `unwrap_or`, `if let`, or explicit error returns.
2. **NO dead code** — No unused variables, parameters, or imports. Code must compile with zero warnings. `#[allow(dead_code)]` is not a fix.
3. **NO incomplete implementations** — `todo!()`, `unimplemented!()`, placeholder returns, and empty match arms are BANNED. Every code path must be fully implemented.
4. **Business logic must be verifiable** — All code must pass `cargo check`. No speculative interfaces, no pseudo-implementations, no "will fix later" stubs.
5. **Validate with `cargo check` and `cargo fix`** — Do NOT use `cargo run` or `cargo build` for validation during development. Check correctness first.
6. **Explicit API and error handling** — Validate all external inputs at boundaries. Never use panic as a substitute for error branches. Return typed errors.
7. **Minimize allocations and copies** — Follow ownership and borrowing best practices. Clone only when necessary (async move, cross-thread). Prefer `&str` over `String`, `Cow` over clone, `Arc` over deep copy.

## Safety Rules

### Error Handling
- `?` with `.context("msg")` preferred
- Never silently swallow errors — log before `.ok()` or `.unwrap_or()`
- `tracing::warn!()` when intentionally discarding errors

### Mutex
- Sync: `parking_lot::Mutex` (no poison, no unwrap)
- Async: `tokio::sync::Mutex` (.lock().await)
- BANNED: `std::sync::Mutex` in production (poisons on panic)

### SQL Safety
- Parameterized queries only: `sqlx::query("...WHERE id = $1").bind(id)`
- Validate dynamic identifiers: `^[a-zA-Z_][a-zA-Z0-9_]{0,62}$`

### Unsafe
- Every `unsafe` block requires `// SAFETY:` comment
- Validate inputs BEFORE the unsafe block

### Logging
- NEVER log tokens, API keys, passwords, auth headers
- Sanitize URLs before logging

### Strings
- Prefer `&str` over `String` in function params
- `Cow<', str>` to avoid unnecessary cloning
- `Arc<str>` for shared immutable strings

## Commit Style
```
feat(scope): description
fix(scope): description
refactor(scope): description
```
English only in commits and code comments.
