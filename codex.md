# codex.md — OpenPRX Rust Production Standards

## Rust Edition: 2024

## Build
```bash
cargo check --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

## Seven Iron Rules
1. NO .unwrap()/.expect() in production — use ?, unwrap_or, if let, explicit error returns
2. NO dead code — zero unused variables/params/imports, zero warnings
3. NO todo!()/unimplemented!()/placeholder returns/empty match arms
4. All code must pass cargo check — no speculative interfaces
5. Validate with cargo check + cargo fix
6. Explicit error handling — validate inputs, never panic for errors
7. Minimize allocations — &str > String, Cow > clone, Arc > deep copy

## More Rules
- parking_lot::Mutex (sync), tokio::sync::Mutex (async) — NEVER std::sync::Mutex
- Parameterized SQL only (sqlx bind)
- Every unsafe needs // SAFETY: comment
- Never log secrets/tokens
- English in code and commits
