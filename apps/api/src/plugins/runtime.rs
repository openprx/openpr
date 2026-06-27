use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wasmtime::{Config, Engine, Instance, Module, Store, StoreLimits, StoreLimitsBuilder};

use super::manifest::PluginRuntimePolicy;

const ABI_VERSION: i32 = 1;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginRuntimeOutput {
    pub output: Value,
    pub duration_ms: u64,
    pub fuel_consumed: Option<u64>,
}

#[derive(Debug)]
struct StoreState {
    limits: StoreLimits,
}

pub async fn invoke_wasm_plugin(
    wasm_bytes: Vec<u8>,
    input: Value,
    policy: PluginRuntimePolicy,
) -> Result<PluginRuntimeOutput, String> {
    let timeout = Duration::from_millis(policy.timeout_ms);
    let timeout_ms = policy.timeout_ms;
    let task = tokio::task::spawn_blocking(move || invoke_wasm_plugin_sync(&wasm_bytes, &input, &policy));
    match tokio::time::timeout(timeout, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => Err(format!("wasm task failed: {err}")),
        Err(_) => Err(format!("wasm execution timeout after {timeout_ms}ms")),
    }
}

pub fn validate_wasm_module(wasm_bytes: &[u8]) -> Result<(), String> {
    let engine = build_engine()?;
    Module::new(&engine, wasm_bytes).map_err(|err| format!("invalid wasm module: {err}"))?;
    Ok(())
}

fn invoke_wasm_plugin_sync(
    wasm_bytes: &[u8],
    input: &Value,
    policy: &PluginRuntimePolicy,
) -> Result<PluginRuntimeOutput, String> {
    let started = Instant::now();
    let engine = build_engine()?;
    let module = Module::new(&engine, wasm_bytes).map_err(|err| format!("invalid wasm module: {err}"))?;
    let limits = StoreLimitsBuilder::new()
        .memory_size(policy.memory_bytes)
        .instances(1)
        .memories(1)
        .tables(1)
        .build();
    let mut store = Store::new(&engine, StoreState { limits });
    store.limiter(|state| &mut state.limits);
    store
        .set_fuel(policy.fuel)
        .map_err(|err| format!("failed to set wasm fuel: {err}"))?;

    let instance =
        Instance::new(&mut store, &module, &[]).map_err(|err| format!("failed to instantiate wasm: {err}"))?;

    if let Ok(version_fn) = instance.get_typed_func::<(), i32>(&mut store, "openpr_plugin_abi_version") {
        let version = version_fn
            .call(&mut store, ())
            .map_err(|err| format!("failed to read plugin abi version: {err}"))?;
        if version != ABI_VERSION {
            return Err(format!("unsupported plugin abi version: {version}"));
        }
    }

    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, "openpr_alloc")
        .map_err(|_| "plugin must export openpr_alloc(len: i32) -> i32".to_string())?;
    let invoke = instance
        .get_typed_func::<(i32, i32), i64>(&mut store, "openpr_invoke")
        .map_err(|_| "plugin must export openpr_invoke(ptr: i32, len: i32) -> i64".to_string())?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| "plugin must export memory".to_string())?;

    let input_bytes = serde_json::to_vec(input).map_err(|err| format!("failed to encode plugin input: {err}"))?;
    let input_len = i32::try_from(input_bytes.len()).map_err(|_| "plugin input is too large".to_string())?;
    let input_ptr = alloc
        .call(&mut store, input_len)
        .map_err(|err| format!("plugin allocation failed: {err}"))?;
    let input_offset = usize::try_from(input_ptr).map_err(|_| "plugin returned negative input pointer".to_string())?;
    memory
        .write(&mut store, input_offset, &input_bytes)
        .map_err(|err| format!("failed to write plugin input: {err}"))?;

    let output_handle = invoke
        .call(&mut store, (input_ptr, input_len))
        .map_err(|err| format!("plugin invocation trapped: {err}"))?;
    let (output_ptr, output_len) = unpack_ptr_len(output_handle)?;
    if output_len > MAX_OUTPUT_BYTES {
        return Err("plugin output is too large".to_string());
    }
    let mut output_bytes = vec![0_u8; output_len];
    memory
        .read(&store, output_ptr, &mut output_bytes)
        .map_err(|err| format!("failed to read plugin output: {err}"))?;
    let output = serde_json::from_slice(&output_bytes).map_err(|err| format!("plugin output is not JSON: {err}"))?;
    let fuel_remaining = store.get_fuel().ok();

    Ok(PluginRuntimeOutput {
        output,
        duration_ms: millis_u64(started.elapsed()),
        fuel_consumed: fuel_remaining.map(|remaining| policy.fuel.saturating_sub(remaining)),
    })
}

fn build_engine() -> Result<Engine, String> {
    let mut config = Config::new();
    config.consume_fuel(true);
    Engine::new(&config).map_err(|err| format!("failed to build wasm engine: {err}"))
}

fn unpack_ptr_len(handle: i64) -> Result<(usize, usize), String> {
    let raw = u64::try_from(handle).map_err(|_| "plugin returned negative output handle".to_string())?;
    let ptr = usize::try_from(raw >> 32).map_err(|_| "plugin output pointer is too large".to_string())?;
    let len = usize::try_from(raw & 0xffff_ffff).map_err(|_| "plugin output length is too large".to_string())?;
    Ok((ptr, len))
}

fn millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{PluginRuntimePolicy, invoke_wasm_plugin, validate_wasm_module};
    use serde_json::json;

    fn echo_ok_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
              (memory (export "memory") 1)
              (global $heap (mut i32) (i32.const 4096))
              (data (i32.const 1024) "{\"ok\":true,\"total\":\"0.30\"}")
              (func (export "openpr_plugin_abi_version") (result i32)
                i32.const 1)
              (func (export "openpr_alloc") (param $len i32) (result i32)
                (local $ptr i32)
                global.get $heap
                local.set $ptr
                global.get $heap
                local.get $len
                i32.add
                global.set $heap
                local.get $ptr)
              (func (export "openpr_invoke") (param $ptr i32) (param $len i32) (result i64)
                i64.const 1024
                i64.const 32
                i64.shl
                i64.const 26
                i64.or))
            "#,
        )
        .expect("wat should compile")
    }

    #[tokio::test]
    async fn invokes_core_wasm_abi_and_reads_json_output() {
        let wasm = echo_ok_wasm();
        validate_wasm_module(&wasm).expect("module should validate");
        let output = invoke_wasm_plugin(
            wasm,
            json!({"amount": "0.30"}),
            PluginRuntimePolicy {
                timeout_ms: 500,
                fuel: 100_000,
                memory_bytes: 1024 * 1024,
            },
        )
        .await
        .expect("wasm should run");

        assert_eq!(output.output.get("ok").and_then(serde_json::Value::as_bool), Some(true));
        assert_eq!(
            output.output.get("total").and_then(serde_json::Value::as_str),
            Some("0.30")
        );
        assert!(output.fuel_consumed.unwrap_or(0) > 0);
    }

    #[tokio::test]
    async fn rejects_wasm_without_required_abi_exports() {
        let wasm = wat::parse_str("(module)").expect("wat should compile");
        let err = invoke_wasm_plugin(wasm, json!({}), PluginRuntimePolicy::default())
            .await
            .expect_err("missing abi should fail");

        assert!(err.contains("openpr_alloc"));
    }

    #[tokio::test]
    async fn rejects_wasm_imports_so_plugins_have_no_host_io() {
        let wasm = wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "fd_write" (func $fd_write))
              (memory (export "memory") 1)
              (func (export "openpr_alloc") (param i32) (result i32) i32.const 0)
              (func (export "openpr_invoke") (param i32) (param i32) (result i64)
                call $fd_write
                i64.const 0))
            "#,
        )
        .expect("wat should compile");
        let err = invoke_wasm_plugin(wasm, json!({}), PluginRuntimePolicy::default())
            .await
            .expect_err("imports should fail");

        assert!(err.contains("failed to instantiate wasm"));
    }

    #[tokio::test]
    async fn fuel_limit_traps_runaway_plugins() {
        let wasm = wat::parse_str(
            r#"
            (module
              (memory (export "memory") 1)
              (func (export "openpr_alloc") (param i32) (result i32) i32.const 0)
              (func (export "openpr_invoke") (param i32) (param i32) (result i64)
                (loop $again
                  br $again)
                i64.const 0))
            "#,
        )
        .expect("wat should compile");
        let err = invoke_wasm_plugin(
            wasm,
            json!({}),
            PluginRuntimePolicy {
                timeout_ms: 500,
                fuel: 10,
                memory_bytes: 1024 * 1024,
            },
        )
        .await
        .expect_err("fuel should trap");

        assert!(err.contains("plugin invocation trapped"));
    }
}
