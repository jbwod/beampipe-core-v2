//! WASM extension host for project config hooks.
//!
//! Guest modules should export `memory`, `alloc(size: i32) -> i32`, and hook functions
//! named `prepare_metadata`, `manifest`, or `graph_patches` with signature
//! `(input_ptr: i32, input_len: i32) -> i64` (high 32 bits = output length, low 32 = output ptr).

use crate::ExtensionHook;
use serde_json::Value;
use std::sync::Arc;
use thiserror::Error;
use wasmtime::{Engine, Instance, Linker, Module, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    PrepareMetadata,
    Manifest,
    GraphPatches,
}

impl HookKind {
    pub fn export_name(self) -> &'static str {
        match self {
            Self::PrepareMetadata => "prepare_metadata",
            Self::Manifest => "manifest",
            Self::GraphPatches => "graph_patches",
        }
    }

    pub fn from_hook_name(name: &str) -> Option<Self> {
        match name {
            "prepare_metadata" => Some(Self::PrepareMetadata),
            "manifest" => Some(Self::Manifest),
            "graph_patches" => Some(Self::GraphPatches),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum WasmHostError {
    #[error("wasm engine error: {0}")]
    Engine(#[from] wasmtime::Error),
    #[error("wasm module bytes are empty")]
    EmptyModule,
    #[error("hook export not found: {0}")]
    MissingExport(&'static str),
    #[error("invalid hook output JSON: {0}")]
    InvalidOutput(#[from] serde_json::Error),
    #[error("hook returned error: {0}")]
    HookFailed(String),
    #[error("hook output read failed")]
    OutputRead,
}

#[derive(Default)]
pub struct WasmHost {
    engine: Engine,
}

impl WasmHost {
    pub fn validate_module(&self, bytes: &[u8]) -> Result<(), WasmHostError> {
        if bytes.is_empty() {
            return Err(WasmHostError::EmptyModule);
        }
        let _module = Module::new(&self.engine, bytes)?;
        Ok(())
    }

    pub fn instantiate(&self, bytes: &[u8]) -> Result<Store<()>, WasmHostError> {
        if bytes.is_empty() {
            return Err(WasmHostError::EmptyModule);
        }
        let module = Module::new(&self.engine, bytes)?;
        let mut store = Store::new(&self.engine, ());
        let _instance = Instance::new(&mut store, &module, &[])?;
        Ok(store)
    }

    pub fn call_hook(
        &self,
        bytes: &[u8],
        hook: HookKind,
        input: &Value,
    ) -> Result<Value, WasmHostError> {
        if bytes.is_empty() {
            return Err(WasmHostError::EmptyModule);
        }
        let module = Module::new(&self.engine, bytes)?;
        let mut store = Store::new(&self.engine, ());
        store.set_fuel(u64::MAX).ok();
        let linker = Linker::new(&self.engine);
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(WasmHostError::Engine)?;

        let input_bytes = serde_json::to_vec(input).map_err(WasmHostError::InvalidOutput)?;
        let output_bytes =
            invoke_string_hook(&mut store, &instance, hook.export_name(), &input_bytes)?;
        serde_json::from_slice(&output_bytes).map_err(WasmHostError::InvalidOutput)
    }

    pub fn maybe_apply_hook(
        &self,
        bytes: Option<&[u8]>,
        hooks: &[ExtensionHook],
        hook: HookKind,
        declarative: &Value,
        envelope: &Value,
    ) -> Result<Value, WasmHostError> {
        let hook_name = hook.export_name();
        if !hooks.iter().any(|h| h.as_str() == hook_name) {
            return Ok(declarative.clone());
        }
        let Some(bytes) = bytes else {
            return Ok(declarative.clone());
        };
        let mut input = envelope.clone();
        if let Some(obj) = input.as_object_mut() {
            obj.insert("declarative_output".into(), declarative.clone());
        }
        self.call_hook(bytes, hook, &input)
    }
}

fn invoke_string_hook(
    store: &mut Store<()>,
    instance: &Instance,
    export_name: &str,
    input: &[u8],
) -> Result<Vec<u8>, WasmHostError> {
    let memory = instance
        .get_memory(&mut *store, "memory")
        .ok_or(WasmHostError::MissingExport("memory"))?;
    let alloc = instance
        .get_typed_func::<i32, i32>(&mut *store, "alloc")
        .map_err(|_| WasmHostError::MissingExport("alloc"))?;
    let hook_fn = instance
        .get_typed_func::<(i32, i32), i64>(&mut *store, export_name)
        .map_err(|_| {
            WasmHostError::MissingExport(match export_name {
                "prepare_metadata" => "prepare_metadata",
                "manifest" => "manifest",
                _ => "graph_patches",
            })
        })?;

    let input_ptr = alloc.call(&mut *store, input.len() as i32)?;
    memory
        .write(&mut *store, input_ptr as usize, input)
        .map_err(|e| WasmHostError::Engine(e.into()))?;
    let packed = hook_fn.call(&mut *store, (input_ptr, input.len() as i32))?;
    let out_ptr = (packed & 0xFFFF_FFFF) as u32 as i32;
    let out_len = (packed >> 32) as u32 as usize;
    if out_len == 0 {
        return Err(WasmHostError::OutputRead);
    }
    let mut buf = vec![0u8; out_len];
    memory
        .read(&mut *store, out_ptr as usize, &mut buf)
        .map_err(|e| WasmHostError::Engine(e.into()))?;
    Ok(buf)
}

pub fn shared_host() -> Arc<WasmHost> {
    Arc::new(WasmHost::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_wasm_bytes() {
        let host = WasmHost::default();
        assert!(host.validate_module(&[]).is_err());
    }

    #[test]
    fn hook_kind_names() {
        assert_eq!(HookKind::PrepareMetadata.export_name(), "prepare_metadata");
        assert_eq!(
            HookKind::from_hook_name("manifest"),
            Some(HookKind::Manifest)
        );
    }
}
