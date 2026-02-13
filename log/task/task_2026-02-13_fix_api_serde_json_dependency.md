# 任务：修复 API 编译错误（serde_json 未链接）

## 上下文/动机
- `apps/api/src/main.rs` 使用了 `serde_json::json!`，但 `apps/api/Cargo.toml` 未声明 `serde_json` 依赖，导致 `E0433`。

## 结论与方案
- 在 `apps/api/Cargo.toml` 添加 `serde_json.workspace = true`。
- 重新执行 `cargo check -p api` 验证修复。

## 涉及文件
- `apps/api/Cargo.toml`
- `log/task/task_2026-02-13_fix_api_serde_json_dependency.md`
- `log/done/fix_2026-02-13_api_add_serde_json_dependency.md`
- `log/changelog.md`

## 计划
- [x] 补充依赖声明
- [x] 运行编译校验

## 已完成
- [x] 已添加 `serde_json.workspace = true` 到 `apps/api/Cargo.toml`。
- [x] `cargo check -p api` 已通过。

## 未完成
- [ ] 运行全仓 `cargo check --workspace --all-targets`（建议在网络/缓存稳定时执行）。

## 对应 done/ 文档索引
- `log/done/fix_2026-02-13_api_add_serde_json_dependency.md`
