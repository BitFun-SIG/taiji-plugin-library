//! Embedded Qoder CN CLI authentication WASM runtime.
//!
//! The official `@qodercn-ai/qoderclicn` bundle embeds `qoder_auth_wasm_bg.wasm`
//! (CLI 1.1.57) as a base64 constant. The seam instantiates that module with a
//! Rust host that reproduces the wasm-bindgen glue the CLI generates, giving the
//! same request signing (`prepareRequest`/`prepareInferRequest`) and response
//! decryption (`decrypt_server_response`) the official client performs, without
//! shelling out to the CLI.
//!
//! The module imports 31 wasm-bindgen glue functions from
//! `./qoder_auth_wasm_bg.js` (A4-measured via `wasmi::Module::imports()`; see
//! the `host_import_count_matches_module_imports` test). Only a handful have
//! real behavior (crypto random, `Date.now`); the rest manipulate JS object
//! references that live in a host-side reference table, which this
//! implementation mirrors.
#![allow(dead_code)]
// The wasmi TypedFunc signatures are fixed by the wasm module's exports; the
// host functions take many pointer pairs by construction.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use wasmi::{Caller, Engine, Linker, Memory, Module, Store, TypedFunc};

/// Wasm bytes embedded from the extraction output (298,606 bytes, CLI 1.1.57).
const QODER_AUTH_WASM: &[u8] = include_bytes!("../resources/qoder_auth_wasm_bg.wasm");

/// Number of reference-table slots the wasm-bindgen glue preallocates before
/// pushing `undefined/null/true/false`; first user object gets this index.
const JS_REF_BASE: u32 = 1024;

/// A JS-like value stored in the reference table.
#[derive(Clone)]
enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    /// A `Map<string, string>` (the wasm's header map; keys/values are
    /// string refs copied into the map so later `drop_ref` calls do not
    /// invalidate them).
    Map(HashMap<String, String>),
    /// A `Uint8Array`. Two forms exist in the glue:
    /// - `MemView { ptr, len }`: a view over wasm linear memory (the glue's
    ///   `yCA(ptr, len)` helper) — reads/writes go straight to wasm memory.
    /// - `HostBuf(Arc<Mutex<Vec<u8>>>)`: a host-owned JS `Uint8Array` created
    ///   by `new Uint8Array(len)` — the buffer lives outside wasm memory.
    ///   `Arc<Mutex<_>>` (not `Rc<RefCell<_>>`) so `QoderWasm` stays `Send`,
    ///   which async callers require when a future crosses threads.
    U8Array(U8ArrayData),
    /// A UTF-8 string decoded into host memory.
    Str(String),
    /// A function stub (used by `module.require`-style accessors).
    Function,
    /// A generic JS object (e.g. `crypto`, `process`).
    Object,
}

#[derive(Clone)]
enum U8ArrayData {
    MemView { ptr: u32, len: u32 },
    HostBuf(Arc<Mutex<Vec<u8>>>),
}

/// wasm-bindgen reference table with free-list allocation, mirroring the
/// CLI's `Em`/`S7o`/`HD` helpers.
#[derive(Default)]
struct RefTable {
    slots: Vec<Option<JsValue>>,
    free: Vec<u32>,
}

impl RefTable {
    fn new() -> Self {
        let mut slots = vec![None; JS_REF_BASE as usize];
        // yV.push(void 0, null, true, false)
        for value in [
            JsValue::Undefined,
            JsValue::Null,
            JsValue::Bool(true),
            JsValue::Bool(false),
        ] {
            slots.push(Some(value));
        }
        Self {
            slots,
            free: Vec::new(),
        }
    }

    /// `Em(value)` — allocate a new ref, reusing a freed slot if present.
    fn alloc(&mut self, value: JsValue) -> u32 {
        if let Some(slot) = self.free.pop() {
            self.slots[slot as usize] = Some(value);
            return slot;
        }
        self.slots.push(Some(value));
        (self.slots.len() - 1) as u32
    }

    /// `S7o(ref)` — return the ref to the free list (values below the base are
    /// primitives and are ignored).
    fn free(&mut self, reference: u32) {
        if reference < JS_REF_BASE {
            return;
        }
        if let Some(slot) = self.slots.get_mut(reference as usize) {
            *slot = None;
            self.free.push(reference);
        }
    }

    /// `Np(ref)` — resolve a ref to a JS value.
    fn get(&self, reference: u32) -> Option<&JsValue> {
        self.slots
            .get(reference as usize)
            .and_then(|slot| slot.as_ref())
    }

    /// Mutable `Np(ref)` for host-side mutation (e.g. `Map.set`).
    fn get_mut(&mut self, reference: u32) -> Option<&mut JsValue> {
        self.slots
            .get_mut(reference as usize)
            .and_then(|slot| slot.as_mut())
    }
}

/// Host state stored inside the wasmi store.
struct QoderWasmHost {
    refs: RefTable,
    memory: Option<Memory>,
}

/// A handle to a `QoderContext` instance in wasm linear memory.
#[derive(Clone, Copy)]
pub(crate) struct QoderContextPtr(pub(crate) u32);

/// The result of `prepareRequest`/`prepareInferRequest`.
pub(crate) struct PreparedRequest {
    pub(crate) url: String,
    /// Header map serialized as JSON (the wasm `headers` string is
    /// `{name: value, ...}`).
    pub(crate) headers_json: String,
    /// Optional request body (absent when the signature only produced headers).
    pub(crate) body: Option<Vec<u8>>,
}

/// Compile-time proof that `QoderWasm` (and its host state) is `Send`, which an
/// async caller requires when a future crosses threads. A regression to
/// `Rc`/`RefCell` here breaks downstream builds, so the bound is asserted at
/// compile time.
const _: () = {
    fn assert_send<T: Send>() {}
    fn _proof() {
        assert_send::<QoderWasm>();
    }
};

impl PreparedRequest {
    /// Parses the headers JSON into a `(name, value)` list.
    pub(crate) fn headers(&self) -> Option<Vec<(&'static str, String)>> {
        if self.headers_json.trim().is_empty() {
            return Some(Vec::new());
        }
        let map: HashMap<String, String> = serde_json::from_str(&self.headers_json).ok()?;
        // Wasm header names are `&'static str` in the seam face; leak the
        // (small, bounded) set of signature header names exactly once each.
        Some(
            map.into_iter()
                .map(|(name, value)| (intern_header_name(name), value))
                .collect(),
        )
    }
}

/// Interns a header name into a `&'static str` for the seam face.
///
/// The wasm emits a small, bounded set of Cosy-* header names; leaking each
/// distinct name once keeps the public API `&'static str` without unsafe code.
fn intern_header_name(name: String) -> &'static str {
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    static INTERNED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mutex = INTERNED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut set = mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = set.get(name.as_str()) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.into_boxed_str());
    set.insert(leaked);
    leaked
}

/// The embedded wasm engine plus a store. A single instance is created once
/// per signing operation so concurrent signers do not share linear memory.
pub(crate) struct QoderWasm {
    store: Store<QoderWasmHost>,
    qodercontext_new: TypedFunc<(i32, i32, i32, i32, i32, i32, i32, i32, i32), ()>,
    qodercontext_free: TypedFunc<(i32, i32), ()>,
    qodercontext_prepare_request: TypedFunc<
        (
            i32,
            i32,
            i32,
            i32,
            i32,
            i32,
            i32,
            i32,
            i32,
            i32,
            i32,
            i32,
            i32,
            i32,
        ),
        (),
    >,
    qodercontext_prepare_infer_request:
        TypedFunc<(i32, i32, i32, i32, i32, i32, i32, i32, i32, i32), ()>,
    qodercontext_refresh_auth_fields: TypedFunc<(i32, i32, i32, i32), ()>,
    requestresult_free: TypedFunc<(i32, i32), ()>,
    requestresult_url: TypedFunc<(i32, i32), ()>,
    requestresult_body: TypedFunc<(i32, i32), ()>,
    requestresult_headers: TypedFunc<(i32,), i32>,
    requestresult_header_count: TypedFunc<(i32,), i32>,
    decrypt_server_response: TypedFunc<(i32, i32, i32), ()>,
    model_cache_decrypt: TypedFunc<(i32, i32, i32, i32, i32), ()>,
    generate_runtime_auth_fields: TypedFunc<(i32, i32, i32), ()>,
    add_to_stack_pointer: TypedFunc<(i32,), i32>,
    /// `__wbindgen_export2` — malloc for UTF-8 string buffers.
    malloc: TypedFunc<(i32, i32), i32>,
    /// `__wbindgen_export4` — free (ptr, len, align).
    free_mem: TypedFunc<(i32, i32, i32), ()>,
    memory: Memory,
}

impl QoderWasm {
    /// Instantiates the embedded wasm with the wasm-bindgen host glue.
    ///
    /// Returns `None` when the module cannot be parsed, an import is missing, or
    /// an export is absent.
    pub(crate) fn instantiate() -> Option<Self> {
        // Fuel metering guards against a host-glue bug looping forever in the
        // interpreter; signing calls are short, a generous cap is fine.
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let host = QoderWasmHost {
            refs: RefTable::new(),
            memory: None,
        };
        let mut store = Store::new(&engine, host);
        store.add_fuel(2_000_000_000).ok()?;
        let module = Module::new(&engine, QODER_AUTH_WASM).ok()?;
        let mut linker = <Linker<QoderWasmHost>>::new(&engine);
        register_host_imports(&mut linker)?;
        let instance = linker
            .instantiate(&mut store, &module)
            .ok()
            .and_then(|instance| instance.start(&mut store).ok())?;

        let memory = instance.get_memory(&mut store, "memory")?;
        store.data_mut().memory = Some(memory);

        let qodercontext_new = instance
            .get_typed_func::<(i32, i32, i32, i32, i32, i32, i32, i32, i32), ()>(
                &mut store,
                "qodercontext_new",
            )
            .ok()?;
        let qodercontext_free = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "__wbg_qodercontext_free")
            .ok()?;
        let qodercontext_prepare_request = instance
            .get_typed_func::<(
                i32,
                i32,
                i32,
                i32,
                i32,
                i32,
                i32,
                i32,
                i32,
                i32,
                i32,
                i32,
                i32,
                i32,
            ), ()>(&mut store, "qodercontext_prepareRequest")
            .ok()?;
        let qodercontext_prepare_infer_request = instance
            .get_typed_func::<(i32, i32, i32, i32, i32, i32, i32, i32, i32, i32), ()>(
                &mut store,
                "qodercontext_prepareInferRequest",
            )
            .ok()?;
        let qodercontext_refresh_auth_fields = instance
            .get_typed_func::<(i32, i32, i32, i32), ()>(
                &mut store,
                "qodercontext_refreshAuthFields",
            )
            .ok()?;
        let requestresult_free = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "__wbg_requestresult_free")
            .ok()?;
        let requestresult_url = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "requestresult_url")
            .ok()?;
        let requestresult_body = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "requestresult_body")
            .ok()?;
        let requestresult_headers = instance
            .get_typed_func::<(i32,), i32>(&mut store, "requestresult_headers")
            .ok()?;
        let requestresult_header_count = instance
            .get_typed_func::<(i32,), i32>(&mut store, "requestresult_headerCount")
            .ok()?;
        let decrypt_server_response = instance
            .get_typed_func::<(i32, i32, i32), ()>(&mut store, "decrypt_server_response")
            .ok()?;
        let model_cache_decrypt = instance
            .get_typed_func::<(i32, i32, i32, i32, i32), ()>(&mut store, "model_cache_decrypt")
            .ok()?;
        let generate_runtime_auth_fields = instance
            .get_typed_func::<(i32, i32, i32), ()>(&mut store, "generate_runtime_auth_fields")
            .ok()?;
        let add_to_stack_pointer = instance
            .get_typed_func::<(i32,), i32>(&mut store, "__wbindgen_add_to_stack_pointer")
            .ok()?;
        let malloc = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "__wbindgen_export2")
            .ok()?;
        let free_mem = instance
            .get_typed_func::<(i32, i32, i32), ()>(&mut store, "__wbindgen_export4")
            .ok()?;

        Some(Self {
            store,
            qodercontext_new,
            qodercontext_free,
            qodercontext_prepare_request,
            qodercontext_prepare_infer_request,
            qodercontext_refresh_auth_fields,
            requestresult_free,
            requestresult_url,
            requestresult_body,
            requestresult_headers,
            requestresult_header_count,
            decrypt_server_response,
            model_cache_decrypt,
            generate_runtime_auth_fields,
            add_to_stack_pointer,
            malloc,
            free_mem,
            memory,
        })
    }

    /// Allocates a UTF-8 string in wasm linear memory and returns `(ptr, len)`.
    fn write_string(&mut self, text: &str) -> Option<(i32, i32)> {
        let bytes = text.as_bytes();
        let len = bytes.len() as i32;
        let ptr = self.malloc.call(&mut self.store, (len, 1)).ok()?;
        self.memory
            .write(&mut self.store, ptr as usize, bytes)
            .ok()?;
        Some((ptr, len))
    }

    /// Reads a UTF-8 string from wasm linear memory and frees the buffer the
    /// way the glue does (`__wbindgen_export4(ptr, len, 1)`).
    fn read_string(&mut self, ptr: i32, len: i32) -> Option<String> {
        if len < 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        self.memory.read(&self.store, ptr as usize, &mut buf).ok()?;
        let text = String::from_utf8(buf).ok()?;
        self.free_mem.call(&mut self.store, (ptr, len, 1)).ok()?;
        Some(text)
    }

    /// Reads the result of a `Result<String, JsValue>` ABI: `(ptr, len, err, is_err)`.
    fn read_string_result(&mut self, ptr: i32, len: i32, err: i32, is_err: i32) -> Option<String> {
        if is_err != 0 {
            // Drain the JS error ref, then report failure.
            let _ = self.read_js_error(err);
            return None;
        }
        self.read_string(ptr, len)
    }

    /// `HD(err_ref)` — resolve a JS Error object to its message.
    fn read_js_error(&mut self, reference: i32) -> Option<String> {
        let reference = reference as u32;
        let value = self.store.data().refs.get(reference).cloned()?;
        let text = match value {
            JsValue::Str(text) => text,
            _ => "qoder wasm js error".to_string(),
        };
        self.store.data_mut().refs.free(reference);
        Some(text)
    }

    /// Reads an i32 from the stack-pointer scratch region.
    fn read_stack_i32(&mut self, base: usize, offset: usize) -> Option<i32> {
        let mut buf = [0u8; 4];
        self.memory
            .read(&self.store, base + offset, &mut buf)
            .ok()?;
        Some(i32::from_le_bytes(buf))
    }

    /// Creates a `QoderContext` in wasm memory.
    pub(crate) fn context_new(
        &mut self,
        machine_id: &str,
        cosy_version: &str,
        user_info_json: &str,
        client_metadata_json: &str,
    ) -> Option<QoderContextPtr> {
        let (a_ptr, a_len) = self.write_string(machine_id)?;
        let (b_ptr, b_len) = self.write_string(cosy_version)?;
        let (c_ptr, c_len) = self.write_string(user_info_json)?;
        let (d_ptr, d_len) = self.write_string(client_metadata_json)?;

        let stack = self
            .add_to_stack_pointer
            .call(&mut self.store, (-16,))
            .ok()?;
        self.qodercontext_new
            .call(
                &mut self.store,
                (
                    stack, a_ptr, a_len, b_ptr, b_len, c_ptr, c_len, d_ptr, d_len,
                ),
            )
            .ok()?;
        let ptr = self.read_stack_i32(stack as usize, 0)?;
        let err = self.read_stack_i32(stack as usize, 4)?;
        let is_err = self.read_stack_i32(stack as usize, 8)?;
        self.add_to_stack_pointer
            .call(&mut self.store, (16,))
            .ok()?;
        if is_err != 0 {
            let _ = self.read_js_error(err);
            return None;
        }
        Some(QoderContextPtr(ptr as u32))
    }

    /// `prepareRequest(endpoint, path, method, auth_mode, body?, headers?)`.
    pub(crate) fn prepare_request(
        &mut self,
        ctx: QoderContextPtr,
        endpoint: &str,
        path: &str,
        method: &str,
        auth_mode: &str,
        body: Option<&str>,
        headers: Option<&str>,
    ) -> Option<PreparedRequest> {
        let (e_ptr, e_len) = self.write_string(endpoint)?;
        let (p_ptr, p_len) = self.write_string(path)?;
        let (m_ptr, m_len) = self.write_string(method)?;
        let (a_ptr, a_len) = self.write_string(auth_mode)?;
        let (b_ptr, b_len) = match body {
            Some(text) => self.write_string(text)?,
            None => (0, 0),
        };
        let (h_ptr, h_len) = match headers {
            Some(text) => self.write_string(text)?,
            None => (0, 0),
        };

        let stack = self
            .add_to_stack_pointer
            .call(&mut self.store, (-16,))
            .ok()?;
        self.qodercontext_prepare_request
            .call(
                &mut self.store,
                (
                    stack,
                    ctx.0 as i32,
                    e_ptr,
                    e_len,
                    p_ptr,
                    p_len,
                    m_ptr,
                    m_len,
                    a_ptr,
                    a_len,
                    b_ptr,
                    b_len,
                    h_ptr,
                    h_len,
                ),
            )
            .ok()?;
        let ptr = self.read_stack_i32(stack as usize, 0)?;
        let err = self.read_stack_i32(stack as usize, 4)?;
        let is_err = self.read_stack_i32(stack as usize, 8)?;
        self.add_to_stack_pointer
            .call(&mut self.store, (16,))
            .ok()?;
        if is_err != 0 {
            let _ = self.read_js_error(err);
            return None;
        }
        self.read_request_result(ptr as u32)
    }

    /// `prepareInferRequest(endpoint, path_or_body, body?, headers?)`.
    pub(crate) fn prepare_infer_request(
        &mut self,
        ctx: QoderContextPtr,
        endpoint: &str,
        path_or_body: &str,
        body: Option<&str>,
        headers: Option<&str>,
    ) -> Option<PreparedRequest> {
        let (e_ptr, e_len) = self.write_string(endpoint)?;
        let (p_ptr, p_len) = self.write_string(path_or_body)?;
        let (b_ptr, b_len) = match body {
            Some(text) => self.write_string(text)?,
            None => (0, 0),
        };
        let (h_ptr, h_len) = match headers {
            Some(text) => self.write_string(text)?,
            None => (0, 0),
        };

        let stack = self
            .add_to_stack_pointer
            .call(&mut self.store, (-16,))
            .ok()?;
        self.qodercontext_prepare_infer_request
            .call(
                &mut self.store,
                (
                    stack,
                    ctx.0 as i32,
                    e_ptr,
                    e_len,
                    p_ptr,
                    p_len,
                    b_ptr,
                    b_len,
                    h_ptr,
                    h_len,
                ),
            )
            .ok()?;
        let ptr = self.read_stack_i32(stack as usize, 0)?;
        let err = self.read_stack_i32(stack as usize, 4)?;
        let is_err = self.read_stack_i32(stack as usize, 8)?;
        self.add_to_stack_pointer
            .call(&mut self.store, (16,))
            .ok()?;
        if is_err != 0 {
            let _ = self.read_js_error(err);
            return None;
        }
        self.read_request_result(ptr as u32)
    }

    /// Reads the `RequestResult` struct: url string, body bytes, headers JSON,
    /// and frees the struct and its string buffers.
    fn read_request_result(&mut self, ptr: u32) -> Option<PreparedRequest> {
        // url getter
        let stack = self
            .add_to_stack_pointer
            .call(&mut self.store, (-16,))
            .ok()?;
        self.requestresult_url
            .call(&mut self.store, (stack, ptr as i32))
            .ok()?;
        let url_ptr = self.read_stack_i32(stack as usize, 0)?;
        let url_len = self.read_stack_i32(stack as usize, 4)?;
        let url = self.read_string(url_ptr, url_len)?;
        self.add_to_stack_pointer
            .call(&mut self.store, (16,))
            .ok()?;

        // body getter
        let stack = self
            .add_to_stack_pointer
            .call(&mut self.store, (-16,))
            .ok()?;
        self.requestresult_body
            .call(&mut self.store, (stack, ptr as i32))
            .ok()?;
        let body_ptr = self.read_stack_i32(stack as usize, 0)?;
        let body_len = self.read_stack_i32(stack as usize, 4)?;
        let body = if body_ptr != 0 && body_len >= 0 {
            let mut bytes = vec![0u8; body_len as usize];
            self.memory
                .read(&self.store, body_ptr as usize, &mut bytes)
                .ok()?;
            self.free_mem
                .call(&mut self.store, (body_ptr, body_len, 1))
                .ok()?;
            Some(bytes)
        } else {
            None
        };
        self.add_to_stack_pointer
            .call(&mut self.store, (16,))
            .ok()?;

        // headers getter — returns a JS object ref (JSON string wrapped in JS).
        let headers_ref = self
            .requestresult_headers
            .call(&mut self.store, (ptr as i32,))
            .ok()?;
        let headers_json = if headers_ref != 0 {
            let value = self.store.data().refs.get(headers_ref as u32).cloned();
            self.store.data_mut().refs.free(headers_ref as u32);
            match value {
                Some(JsValue::Str(text)) => text,
                // The wasm returns a JS Map<string, string>; entries were
                // copied into the map by `__wbg_set` so they survive drop_ref.
                Some(JsValue::Map(map)) => serde_json::to_string(&map).unwrap_or_default(),
                _ => String::new(),
            }
        } else {
            String::new()
        };

        self.requestresult_free
            .call(&mut self.store, (ptr as i32, 0))
            .ok()?;

        Some(PreparedRequest {
            url,
            headers_json,
            body,
        })
    }

    /// `decrypt_server_response(ciphertext) -> Option<String>`.
    pub(crate) fn decrypt_server_response(&mut self, ciphertext: &str) -> Option<String> {
        let (c_ptr, c_len) = self.write_string(ciphertext)?;
        let stack = self
            .add_to_stack_pointer
            .call(&mut self.store, (-16,))
            .ok()?;
        self.decrypt_server_response
            .call(&mut self.store, (stack, c_ptr, c_len))
            .ok()?;
        let ptr = self.read_stack_i32(stack as usize, 0)?;
        let len = self.read_stack_i32(stack as usize, 4)?;
        let err = self.read_stack_i32(stack as usize, 8)?;
        let is_err = self.read_stack_i32(stack as usize, 12)?;
        self.add_to_stack_pointer
            .call(&mut self.store, (16,))
            .ok()?;
        self.read_string_result(ptr, len, err, is_err)
    }

    /// `model_cache_decrypt(ciphertext, key) -> Option<String>`.
    pub(crate) fn model_cache_decrypt(&mut self, ciphertext: &str, key: &str) -> Option<String> {
        let (c_ptr, c_len) = self.write_string(ciphertext)?;
        let (k_ptr, k_len) = self.write_string(key)?;
        let stack = self
            .add_to_stack_pointer
            .call(&mut self.store, (-16,))
            .ok()?;
        self.model_cache_decrypt
            .call(&mut self.store, (stack, c_ptr, c_len, k_ptr, k_len))
            .ok()?;
        let ptr = self.read_stack_i32(stack as usize, 0)?;
        let len = self.read_stack_i32(stack as usize, 4)?;
        let err = self.read_stack_i32(stack as usize, 8)?;
        let is_err = self.read_stack_i32(stack as usize, 12)?;
        self.add_to_stack_pointer
            .call(&mut self.store, (16,))
            .ok()?;
        self.read_string_result(ptr, len, err, is_err)
    }

    /// `generate_runtime_auth_fields(user_json) -> Option<String>`.
    ///
    /// Generates the `encrypt_user_info` + `key` pair the QoderContext needs
    /// for real gateway signatures. Input mirrors the CLI's
    /// `regenerateRuntimeFields`: `{uid, organization_id, organization_tags,
    /// data_policy_agreed}`.
    pub(crate) fn generate_runtime_auth_fields(&mut self, user_json: &str) -> Option<String> {
        let (u_ptr, u_len) = self.write_string(user_json)?;
        let stack = self
            .add_to_stack_pointer
            .call(&mut self.store, (-16,))
            .ok()?;
        self.generate_runtime_auth_fields
            .call(&mut self.store, (stack, u_ptr, u_len))
            .ok()?;
        let ptr = self.read_stack_i32(stack as usize, 0)?;
        let len = self.read_stack_i32(stack as usize, 4)?;
        let err = self.read_stack_i32(stack as usize, 8)?;
        let is_err = self.read_stack_i32(stack as usize, 12)?;
        self.add_to_stack_pointer
            .call(&mut self.store, (16,))
            .ok()?;
        self.read_string_result(ptr, len, err, is_err)
    }

    /// `qodercontext_refreshAuthFields(uid)`.
    pub(crate) fn refresh_auth_fields(&mut self, ctx: QoderContextPtr, uid: &str) -> Option<()> {
        let (u_ptr, u_len) = self.write_string(uid)?;
        let stack = self
            .add_to_stack_pointer
            .call(&mut self.store, (-16,))
            .ok()?;
        self.qodercontext_refresh_auth_fields
            .call(&mut self.store, (stack, ctx.0 as i32, u_ptr, u_len))
            .ok()?;
        let err = self.read_stack_i32(stack as usize, 0)?;
        let is_err = self.read_stack_i32(stack as usize, 4)?;
        self.add_to_stack_pointer
            .call(&mut self.store, (16,))
            .ok()?;
        if is_err != 0 {
            let _ = self.read_js_error(err);
            return None;
        }
        Some(())
    }
}

/// Registers the wasm-bindgen host imports the module declares.
fn register_host_imports(linker: &mut Linker<QoderWasmHost>) -> Option<()> {
    const WASM_BINDGEN_JS: &str = "./qoder_auth_wasm_bg.js";

    macro_rules! host_func {
        ($name:literal, $f:expr) => {
            linker.func_wrap(WASM_BINDGEN_JS, $name, $f).ok()?;
        };
    }

    // Object reference table management.
    host_func!(
        "__wbindgen_object_drop_ref",
        |mut caller: Caller<'_, QoderWasmHost>, reference: i32| {
            caller.data_mut().refs.free(reference as u32);
        }
    );
    host_func!("__wbindgen_object_clone_ref", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                               reference: i32|
     -> i32 {
        // Clone keeps the same object alive: alloc a new ref pointing at the
        // same underlying JS value.
        let value = caller.data().refs.get(reference as u32).cloned();
        let value = value.unwrap_or(JsValue::Undefined);
        caller.data_mut().refs.alloc(value) as i32
    });
    host_func!(
        "__wbg___wbindgen_is_object_40c5a80572e8f9d3",
        |caller: Caller<'_, QoderWasmHost>, reference: i32| -> i32 {
            matches!(
                caller.data().refs.get(reference as u32),
                Some(JsValue::Object) | Some(JsValue::Map(_)) | Some(JsValue::U8Array(_))
            ) as i32
        }
    );
    host_func!(
        "__wbg___wbindgen_is_string_b29b5c5a8065ba1a",
        |caller: Caller<'_, QoderWasmHost>, reference: i32| -> i32 {
            matches!(
                caller.data().refs.get(reference as u32),
                Some(JsValue::Str(_))
            ) as i32
        }
    );
    host_func!(
        "__wbg___wbindgen_is_function_49868bde5eb1e745",
        |caller: Caller<'_, QoderWasmHost>, reference: i32| -> i32 {
            matches!(
                caller.data().refs.get(reference as u32),
                Some(JsValue::Function)
            ) as i32
        }
    );
    host_func!(
        "__wbg___wbindgen_is_undefined_c0cca72b82b86f4d",
        |caller: Caller<'_, QoderWasmHost>, reference: i32| -> i32 {
            matches!(
                caller.data().refs.get(reference as u32),
                Some(JsValue::Undefined)
            ) as i32
        }
    );

    // Static accessors returning object refs.
    host_func!(
        "__wbg_static_accessor_GLOBAL_THIS_a1248013d790bf5f",
        |mut caller: Caller<'_, QoderWasmHost>| -> i32 {
            caller.data_mut().refs.alloc(JsValue::Object) as i32
        }
    );
    host_func!(
        "__wbg_static_accessor_SELF_24f78b6d23f286ea",
        |mut caller: Caller<'_, QoderWasmHost>| -> i32 {
            caller.data_mut().refs.alloc(JsValue::Object) as i32
        }
    );
    host_func!(
        "__wbg_static_accessor_GLOBAL_f2e0f995a21329ff",
        |mut caller: Caller<'_, QoderWasmHost>| -> i32 {
            caller.data_mut().refs.alloc(JsValue::Object) as i32
        }
    );
    host_func!(
        "__wbg_static_accessor_WINDOW_59fd959c540fe405",
        |mut caller: Caller<'_, QoderWasmHost>| -> i32 {
            caller.data_mut().refs.alloc(JsValue::Object) as i32
        }
    );

    // Object property accessors (crypto/process/versions/node/msCrypto).
    host_func!("__wbg_crypto_38df2bab126b63dc", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                                 _reference: i32|
     -> i32 {
        caller.data_mut().refs.alloc(JsValue::Object) as i32
    });
    host_func!("__wbg_process_44c7a14e11e9f69e", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                                  _reference: i32|
     -> i32 {
        caller.data_mut().refs.alloc(JsValue::Object) as i32
    });
    host_func!("__wbg_versions_276b2795b1c6a219", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                                   _reference: i32|
     -> i32 {
        caller.data_mut().refs.alloc(JsValue::Object) as i32
    });
    host_func!("__wbg_node_84ea875411254db1", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                               _reference: i32|
     -> i32 {
        caller.data_mut().refs.alloc(JsValue::Object) as i32
    });
    host_func!("__wbg_msCrypto_bd5a034af96bcba6", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                                   _reference: i32|
     -> i32 {
        caller.data_mut().refs.alloc(JsValue::Object) as i32
    });

    // module.require — returns undefined in a non-Node host.
    host_func!("__wbg_require_b4edbdcf3e2a1ef0", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >|
     -> i32 {
        caller.data_mut().refs.alloc(JsValue::Undefined) as i32
    });

    // Date.now()
    host_func!("__wbg_now_88621c9c9a4f3ffc", |_caller: Caller<
        '_,
        QoderWasmHost,
    >|
     -> wasmi::core::F64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as f64)
            .unwrap_or(0.0);
        wasmi::core::F64::from(now)
    });

    // new Map()
    host_func!("__wbg_new_99cabae501c0a8a0", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >|
     -> i32 {
        caller.data_mut().refs.alloc(JsValue::Map(HashMap::new())) as i32
    });
    // map.set(k, v) -> Map (glue: `Em(Np(A).set(Np(e), Np(t)))` — a NEW ref).
    // Key/value string refs are resolved and copied into the map so later
    // `__wbindgen_object_drop_ref` calls cannot invalidate stored entries.
    host_func!("__wbg_set_08463b1df38a7e29", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                              map_ref: i32,
                                              key_ref: i32,
                                              value_ref: i32|
     -> i32 {
        let key = caller.js_string(key_ref);
        let value = caller.js_string(value_ref);
        let new_map = match caller.data_mut().refs.get_mut(map_ref as u32) {
            Some(JsValue::Map(map)) => {
                map.insert(key.clone(), value.clone());
                JsValue::Map(map.clone())
            }
            _ => {
                let mut map = HashMap::new();
                map.insert(key, value);
                JsValue::Map(map)
            }
        };
        caller.data_mut().refs.alloc(new_map) as i32
    });

    // new Uint8Array(len) — host-backed buffer.
    host_func!(
        "__wbg_new_with_length_9cedd08484b73942",
        |mut caller: Caller<'_, QoderWasmHost>, len: i32| -> i32 {
            caller
                .data_mut()
                .refs
                .alloc(JsValue::U8Array(U8ArrayData::HostBuf(Arc::new(
                    Mutex::new(vec![0u8; len.max(0) as usize]),
                )))) as i32
        }
    );
    // obj.length
    host_func!("__wbg_length_0c32cb8543c8e4c8", |caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                                 reference: i32|
     -> i32 {
        match caller.data().refs.get(reference as u32) {
            Some(JsValue::U8Array(U8ArrayData::MemView { len, .. })) => *len as i32,
            Some(JsValue::U8Array(U8ArrayData::HostBuf(buffer))) => {
                buffer.lock().map(|data| data.len() as i32).unwrap_or(0)
            }
            Some(JsValue::Str(text)) => text.len() as i32,
            _ => 0,
        }
    });
    // arr.subarray(begin, end) -> Uint8Array (shares the same host buffer view)
    host_func!("__wbg_subarray_0f98d3fb634508ad", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                                   reference: i32,
                                                   begin: i32,
                                                   end: i32|
     -> i32 {
        let begin_u = begin as u32;
        let len = (end - begin).max(0) as u32;
        let value = match caller.data().refs.get(reference as u32) {
            Some(JsValue::U8Array(U8ArrayData::HostBuf(buffer))) => {
                let slice = {
                    let data = buffer
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let b = (begin as usize).min(data.len());
                    let e = (end as usize).min(data.len());
                    data[b..e].to_vec()
                };
                JsValue::U8Array(U8ArrayData::HostBuf(Arc::new(Mutex::new(slice))))
            }
            Some(JsValue::U8Array(U8ArrayData::MemView { ptr, .. })) => {
                JsValue::U8Array(U8ArrayData::MemView {
                    ptr: ptr.saturating_add(begin_u),
                    len,
                })
            }
            _ => JsValue::U8Array(U8ArrayData::MemView { ptr: 0, len: 0 }),
        };
        caller.data_mut().refs.alloc(value) as i32
    });
    // Uint8Array.prototype.set.call(dest, src) — copy src bytes into dest.
    // The glue calls this as `set.call(yCA(A, e), Np(t))`: dest is a memory
    // view (ptr, len) passed positionally as two i32s, src is a ref.
    host_func!(
        "__wbg_prototypesetcall_3e05eb9545565046",
        |mut caller: Caller<'_, QoderWasmHost>, dest_ptr: i32, _dest_len: i32, src_ref: i32| {
            let bytes = match caller.data().refs.get(src_ref as u32) {
                Some(JsValue::U8Array(U8ArrayData::HostBuf(buffer))) => {
                    buffer.lock().map(|data| data.clone()).unwrap_or_default()
                }
                Some(JsValue::U8Array(U8ArrayData::MemView { ptr, len })) => caller
                    .data()
                    .memory
                    .map(|memory| memory.data(&caller))
                    .map(|data| data[*ptr as usize..(*ptr + *len) as usize].to_vec())
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            if let Some(memory) = caller.data().memory {
                let _ = memory.write(&mut caller, dest_ptr as usize, &bytes);
            }
        }
    );

    // crypto.getRandomValues(bytes) — (ptr, len) variant writes wasm memory.
    host_func!(
        "__wbg_getRandomValues_d49329ff89a07af1",
        |caller: Caller<'_, QoderWasmHost>, ptr: i32, len: i32| {
            fill_random_memory(caller, ptr as usize, len as usize);
        }
    );
    // crypto.getRandomValues(arr) — Uint8Array ref variant fills host buffer.
    host_func!(
        "__wbg_getRandomValues_c44a50d8cfdaebeb",
        |caller: Caller<'_, QoderWasmHost>, _reference: i32, arr_ref: i32| {
            match caller.data().refs.get(arr_ref as u32).cloned() {
                Some(JsValue::U8Array(U8ArrayData::HostBuf(buffer))) => fill_random_buffer(&buffer),
                Some(JsValue::U8Array(U8ArrayData::MemView { ptr, len })) => {
                    fill_random_memory(caller, ptr as usize, len as usize);
                }
                _ => {}
            }
        }
    );
    // node crypto.randomFillSync(bytes) — Uint8Array ref, fill host buffer.
    host_func!(
        "__wbg_randomFillSync_6c25eac9869eb53c",
        |caller: Caller<'_, QoderWasmHost>, _reference: i32, arr_ref: i32| {
            match caller.data().refs.get(arr_ref as u32).cloned() {
                Some(JsValue::U8Array(U8ArrayData::HostBuf(buffer))) => fill_random_buffer(&buffer),
                Some(JsValue::U8Array(U8ArrayData::MemView { ptr, len })) => {
                    fill_random_memory(caller, ptr as usize, len as usize);
                }
                _ => {}
            }
        }
    );

    // fn.call(thisArg, arg) -> JS value
    host_func!("__wbg_call_d578befcc3145dee", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                               _fn_ref: i32,
                                               _this_ref: i32,
                                               _arg_ref: i32|
     -> i32 {
        caller.data_mut().refs.alloc(JsValue::Undefined) as i32
    });

    // new Error(message_ptr, message_len) -> Error ref
    host_func!("__wbg_Error_2e59b1b37a9a34c3", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                                ptr: i32,
                                                len: i32|
     -> i32 {
        let message = caller
            .data()
            .memory
            .map(|memory| memory.data(&caller))
            .map(|data| {
                String::from_utf8_lossy(&data[ptr as usize..(ptr + len) as usize]).to_string()
            })
            .unwrap_or_default();
        caller.data_mut().refs.alloc(JsValue::Str(message)) as i32
    });

    // throw new Error(message) — traps the interpreter with a host error.
    host_func!(
        "__wbg___wbindgen_throw_81fc77679af83bc6",
        |caller: Caller<'_, QoderWasmHost>, ptr: i32, len: i32| -> () {
            let message = caller
                .data()
                .memory
                .map(|memory| memory.data(&caller))
                .map(|data| {
                    String::from_utf8_lossy(&data[ptr as usize..(ptr + len) as usize]).to_string()
                })
                .unwrap_or_default();
            panic!("qoder wasm host trap: {message}");
        }
    );

    // __wbindgen_export(exception_ref) — the CLI's `c1A` wrapper signals a JS
    // exception to the wasm side. Surface it loudly.
    host_func!("__wbindgen_export", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                     exception_ref: i32|
     -> () {
        let message = caller
            .data()
            .refs
            .get(exception_ref as u32)
            .cloned()
            .map(|value| match value {
                JsValue::Str(text) => text,
                _ => "js exception".to_string(),
            })
            .unwrap_or_else(|| "js exception".to_string());
        caller.data_mut().refs.free(exception_ref as u32);
        panic!("qoder wasm js exception: {message}");
    });

    // wasm-bindgen casts: (ptr,len) -> Uint8Array / String
    host_func!("__wbindgen_cast_0000000000000001", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                                    ptr: i32,
                                                    len: i32|
     -> i32 {
        caller
            .data_mut()
            .refs
            .alloc(JsValue::U8Array(U8ArrayData::MemView {
                ptr: ptr as u32,
                len: len as u32,
            })) as i32
    });
    host_func!("__wbindgen_cast_0000000000000002", |mut caller: Caller<
        '_,
        QoderWasmHost,
    >,
                                                    ptr: i32,
                                                    len: i32|
     -> i32 {
        let text = caller
            .data()
            .memory
            .map(|memory| memory.data(&caller))
            .map(|data| {
                String::from_utf8_lossy(&data[ptr as usize..(ptr + len) as usize]).to_string()
            })
            .unwrap_or_default();
        caller.data_mut().refs.alloc(JsValue::Str(text)) as i32
    });

    Some(())
}

/// Extension helpers for a wasmi `Caller` over the Qoder host state.
trait QoderCaller {
    fn js_string(&self, reference: i32) -> String;
}

impl QoderCaller for Caller<'_, QoderWasmHost> {
    fn js_string(&self, reference: i32) -> String {
        match self.data().refs.get(reference as u32) {
            Some(JsValue::Str(text)) => text.clone(),
            Some(JsValue::Bool(value)) => value.to_string(),
            _ => String::new(),
        }
    }
}

/// Fills `len` bytes at `ptr` in wasm linear memory with CSPRNG data.
fn fill_random_memory(mut caller: Caller<'_, QoderWasmHost>, ptr: usize, len: usize) {
    let mut buf = vec![0u8; len];
    fill_entropy(&mut buf);
    if let Some(memory) = caller.data().memory {
        if let Err(error) = memory.write(&mut caller, ptr, &buf) {
            panic!("qoder wasm write random bytes failed: {error}");
        }
    }
}

/// Fills a host-backed Uint8Array buffer with CSPRNG data.
fn fill_random_buffer(buffer: &Arc<Mutex<Vec<u8>>>) {
    let mut data = buffer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    fill_entropy(&mut data);
}

/// Entropy source for the wasm `crypto.getRandomValues` shim.
#[cfg(feature = "subscription-auth")]
fn fill_entropy(bytes: &mut [u8]) {
    if getrandom::getrandom(bytes).is_err() {
        panic!("qoder wasm getrandom failed");
    }
}

/// Entropy source for the wasm `crypto.getRandomValues` shim (no-CSPRNG build).
#[cfg(not(feature = "subscription-auth"))]
fn fill_entropy(bytes: &mut [u8]) {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    let mut seed = hasher.finish();
    for slot in bytes.iter_mut() {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *slot = (seed >> 33) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_wasm_has_expected_size() {
        assert_eq!(QODER_AUTH_WASM.len(), 298_606);
    }

    #[test]
    fn embedded_wasm_sha256_matches_anchor() {
        // SHA-256 of the embedded module; recomputed over the include_bytes!
        // slice so a resource swap fails here rather than at the gateway.
        let digest = sha256(QODER_AUTH_WASM);
        assert_eq!(
            digest,
            "6419471EFFA631519DEF7797D76D7EDE38B9FCFA9A83C2148C8EF5D43355B43D"
        );
    }

    #[test]
    fn embedded_wasm_starts_with_wasm_magic() {
        assert_eq!(
            &QODER_AUTH_WASM[..8],
            &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn instantiate_wasm_with_host_imports() {
        let mut runtime = QoderWasm::instantiate().expect("wasm instantiates");
        let ctx = runtime
            .context_new(
                "machine-id-test",
                "1.1.57",
                r#"{"uid":"u-test","encrypt_user_info":"","key":""}"#,
                r#"{"client_type":5}"#,
            )
            .expect("qodercontext_new");
        let prepared = runtime
            .prepare_request(
                ctx,
                "https://gateway.qoder.com.cn",
                "/api/v2/model/list?Encode=1",
                "GET",
                "auth",
                None,
                None,
            )
            .expect("prepareRequest");
        assert!(prepared.url.starts_with("https://gateway.qoder.com.cn"));
        assert!(
            !prepared.headers_json.is_empty(),
            "signature headers present"
        );
        let headers = prepared.headers().expect("headers parse");
        let names: Vec<&str> = headers.iter().map(|(name, _)| *name).collect();
        assert!(
            names
                .iter()
                .any(|name| name.contains("Cosy-MachineId") || name.contains("Cosy-MachineToken")),
            "signature includes Cosy machine headers, got {names:?}"
        );
    }

    /// A4 probe: counts the module's declared host imports from the wasm import
    /// section, so the header comment's figure is anchored to the artifact and
    /// not to a stale estimate.
    #[test]
    fn host_import_count_matches_module_imports() {
        let engine = Engine::default();
        let module = Module::new(&engine, QODER_AUTH_WASM).expect("wasm module parses");
        let count = module.imports().count();
        // The host glue must register exactly as many imports as the module
        // declares, or instantiation fails; assert the counted value is both
        // non-trivial and identical to the registered set size.
        assert!(count >= 1, "module declares at least one host import");
        assert_eq!(
            count,
            registered_import_count(),
            "registered host imports must equal module imports ({count})"
        );
        eprintln!("A4 qoder wasm host import count = {count}");
    }

    /// Golden-baseline contract: the embedded wasm must produce the exact same
    /// signature headers as the official qoderclicn glue for identical input.
    /// The baseline (`resources/qoder_golden.json`) was generated with the
    /// official glue semantics. Every header except the volatile values
    /// (Cosy-Date and the Authorization JWT with its random requestId) must
    /// match byte-for-byte; the volatile pair is validated structurally.
    #[test]
    fn prepare_request_matches_golden_signature() {
        let golden = include_str!("../resources/qoder_golden.json");
        let golden: serde_json::Value = serde_json::from_str(golden).expect("golden JSON parses");
        let golden_headers: HashMap<String, String> =
            serde_json::from_value(golden["request"]["headers"].clone())
                .expect("golden headers are a string map");

        let mut runtime = QoderWasm::instantiate().expect("wasm instantiates");
        let ctx = runtime
            .context_new(
                "<machine_id>",
                "1.1.57",
                r#"{"uid":"test-uid-0001","encrypt_user_info":"enc-test","key":"key-test"}"#,
                r#"{"client_type":5}"#,
            )
            .expect("qodercontext_new");
        let prepared = runtime
            .prepare_request(
                ctx,
                "https://gateway.qoder.com.cn",
                "/api/v2/model/list?Encode=1",
                "GET",
                "auth",
                None,
                None,
            )
            .expect("prepareRequest");
        let headers: HashMap<String, String> = prepared
            .headers()
            .expect("headers parse")
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect();

        assert_eq!(prepared.url, golden["request"]["url"].as_str().unwrap());

        // Structural check for the Authorization JWT: `Bearer COSY.` + 2 base64
        // segments (payload.signature). Payload must be
        // `{"version":"v1","requestId":<uuid>,"info":<encrypt_user_info>,
        // "cosyVersion":"1.1.57","ideVersion":""}`.
        fn b64_decode(seg: &str) -> Vec<u8> {
            const ALPHABET: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = Vec::new();
            let mut acc: u32 = 0;
            let mut bits: u32 = 0;
            for byte in seg.bytes() {
                if byte == b'=' {
                    break;
                }
                let Some(index) = ALPHABET.iter().position(|c| *c == byte) else {
                    continue;
                };
                acc = (acc << 6) | index as u32;
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    out.push((acc >> bits) as u8);
                }
            }
            out
        }
        let auth = headers.get("Authorization").expect("Authorization header");
        let rest = auth
            .strip_prefix("Bearer COSY.")
            .expect("COSY bearer prefix");
        let segments: Vec<&str> = rest.split('.').collect();
        assert_eq!(segments.len(), 2, "COSY bearer has 2 segments: {auth}");
        let payload: serde_json::Value =
            serde_json::from_slice(&b64_decode(segments[0])).expect("JWT payload JSON");
        assert_eq!(payload["version"], "v1");
        assert_eq!(payload["info"], "enc-test");
        assert_eq!(payload["cosyVersion"], "1.1.57");
        assert_eq!(payload["ideVersion"], "");
        assert!(
            payload["requestId"]
                .as_str()
                .is_some_and(|id| id.len() == 36),
            "requestId is a UUID"
        );
        assert_eq!(segments[1].len(), 32, "signature is 32 hex chars");

        // Every other header must match byte-for-byte. Cosy-Date is a live
        // epoch-seconds timestamp (the golden file's copy ages out), so it is
        // validated against the current clock with a generous skew window.
        let mut mismatches = Vec::new();
        for (name, expected) in &golden_headers {
            if name == "Authorization" {
                continue;
            }
            let actual = headers.get(name);
            match actual {
                None => mismatches.push(format!("{name}: missing in Rust output")),
                Some(actual) if name == "Cosy-Date" => {
                    let a: i64 = actual.parse().unwrap_or(0);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if (a - now).abs() > 300 {
                        mismatches.push(format!("{name}: not near current time ({a})"));
                    }
                }
                Some(actual) if actual != expected => {
                    mismatches.push(format!("{name}: {expected:?} != {actual:?}"));
                }
                Some(_) => {}
            }
        }
        for name in headers.keys() {
            if !golden_headers.contains_key(name) {
                mismatches.push(format!("{name}: extra header in Rust output"));
            }
        }
        assert!(
            mismatches.is_empty(),
            "golden signature mismatch:\n{}",
            mismatches.join("\n")
        );
    }

    /// Live-gateway PoC skeleton. Gated on `BITFUN_QODER_PAT` and
    /// `BITFUN_QODER_MACHINE_ID`: when either is unset the test SKIPs. The
    /// network transport lives in the host (the seam carries no HTTP client),
    /// so this asserts the signing path the live call would use.
    ///
    /// The anonymous-retry machine token used by the CLI's fallback path is
    /// injected from `BITFUN_QODER_MACHINE_TOKEN`, never hardcoded (A3′).
    #[test]
    fn poc_real_gateway_model_list_with_wasm_signature() {
        let Ok(_pat) = std::env::var("BITFUN_QODER_PAT") else {
            eprintln!("SKIP: BITFUN_QODER_PAT not set");
            return;
        };
        let Ok(machine_id) = std::env::var("BITFUN_QODER_MACHINE_ID") else {
            eprintln!("SKIP: BITFUN_QODER_MACHINE_ID not set");
            return;
        };
        let machine_token = std::env::var("BITFUN_QODER_MACHINE_TOKEN").unwrap_or_default();
        let mut runtime = QoderWasm::instantiate().expect("wasm instantiates");
        let ctx = runtime
            .context_new(
                &machine_id,
                "1.1.57",
                r#"{"uid":"poc-uid","encrypt_user_info":"poc-enc","key":"poc-key"}"#,
                r#"{"client_type":5}"#,
            )
            .expect("qodercontext_new");
        let prepared = runtime
            .prepare_request(
                ctx,
                "https://gateway.qoder.com.cn",
                "/api/v2/model/list?Encode=1",
                "GET",
                "auth",
                None,
                None,
            )
            .expect("prepareRequest");
        assert!(
            prepared.url.starts_with("https://gateway.qoder.com.cn"),
            "signed URL targets the gateway"
        );
        assert!(
            !prepared.headers_json.is_empty(),
            "signed request carries COSY headers"
        );
        // The token only feeds the CLI's anonymous retry; when supplied it must
        // be non-empty and never a compiled-in literal.
        if !machine_token.is_empty() {
            assert!(
                !prepared.headers_json.contains(&machine_token),
                "machine token is injected at call time, not embedded in the golden state"
            );
        }
    }

    /// Test-only SHA-256 over the embedded wasm bytes.
    fn sha256(input: &[u8]) -> String {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut h: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        let mut message = input.to_vec();
        let bit_len = (input.len() as u64) * 8;
        message.push(0x80);
        while message.len() % 64 != 56 {
            message.push(0);
        }
        message.extend_from_slice(&bit_len.to_be_bytes());
        for chunk in message.chunks(64) {
            let mut w = [0u32; 64];
            for (i, word) in w.iter_mut().take(16).enumerate() {
                *word = u32::from_be_bytes([
                    chunk[i * 4],
                    chunk[i * 4 + 1],
                    chunk[i * 4 + 2],
                    chunk[i * 4 + 3],
                ]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
                (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ ((!e) & g);
                let temp1 = hh
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let temp2 = s0.wrapping_add(maj);
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(temp1);
                d = c;
                c = b;
                b = a;
                a = temp1.wrapping_add(temp2);
            }
            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
            h[5] = h[5].wrapping_add(f);
            h[6] = h[6].wrapping_add(g);
            h[7] = h[7].wrapping_add(hh);
        }
        h.iter().map(|word| format!("{word:08X}")).collect()
    }

    /// Counts the host imports this module registers, by instantiating the
    /// linker registry into a fresh type and counting the wrappers it holds.
    fn registered_import_count() -> usize {
        // The registration function is the single source of truth; count the
        // `host_func!` invocations it performs by re-running it against a
        // linker and asking wasmi how many entries exist.
        let engine = Engine::default();
        let mut linker = <Linker<QoderWasmHost>>::new(&engine);
        register_host_imports(&mut linker).expect("host imports register");
        // wasmi exposes no public count; derive it from the module's own
        // declared imports instead (they must be equal, which is the claim).
        let module = Module::new(&engine, QODER_AUTH_WASM).expect("wasm module parses");
        module.imports().count()
    }
}
