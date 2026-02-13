# fix: 修复 api 缺少 serde_json 依赖导致的 E0433

## 背景
- 编译报错：`use of unresolved module or unlinked crate serde_json`。
- 触发位置：`apps/api/src/main.rs` 中 `serde_json::json!` 调用。

## 变更点
- 在 `apps/api/Cargo.toml` 增加：`serde_json.workspace = true`。

## 涉及文件
- `apps/api/Cargo.toml`
- `log/task/task_2026-02-13_fix_api_serde_json_dependency.md`
- `log/done/fix_2026-02-13_api_add_serde_json_dependency.md`
- `log/changelog.md`

## 验证命令与结果
- 命令：`source $HOME/.cargo/env && cargo check -p api`
- 结果：通过（`Finished 'dev' profile ...`）。

## 后续建议
1. 执行一次全仓校验：`cargo check --workspace --all-targets`。
2. 执行 clippy 与测试，确保新增依赖不引入其他问题。
