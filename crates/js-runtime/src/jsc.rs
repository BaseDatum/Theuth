//! [`JsRuntime`] backed by Apple's `JavaScriptCore.framework` C API (`target_os = "ios"` or `feature = "jsc"`):
//! one isolated `JSContextGroup` + `JSGlobalContext` with the shared polyfill(s) and one [`Bundle`] loaded,
//! panic-catching host callbacks, and wall-clock deadline tracking; [`init_platform`].
//!
//! Uses only Apple's public `JavaScriptCore.framework` C API (no private SPI, no V8 / `deno_core` dependency).
//! `JSEvaluateScript` automatically drains the Promise microtask queue before returning to C, which drives
//! both synchronous bundles ([`crate::APPS`]) and microtask-async bundles ([`crate::DOCUMENTS`],
//! [`crate::MERMAID`]).

use std::ffi::CString;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::time::Instant;

use crate::{Bundle, Error, LOAD_DEADLINE, Limits};

/// Web globals every bundle needs (crypto, atob/btoa, timers).
const POLYFILL: &str = include_str!("../js/polyfill.js");
/// Extra web globals for bundles with [`Bundle::web_polyfill`] (TextEncoder/TextDecoder, structuredClone, URL,
/// console).
const POLYFILL_WEB: &str = include_str!("../js/polyfill-web.js");
/// Bare JavaScriptCore global contexts do not define `queueMicrotask`; supply it over the native Promise
/// microtask queue so bundles that check `typeof queueMicrotask === "function"` (e.g. mermaid's `raf` helper
/// and linkedom/documents) schedule on JSC's microtask queue rather than falling back to `setTimeout`.
const BOOTSTRAP: &str = r#"if (typeof globalThis.queueMicrotask === "undefined") {
  globalThis.queueMicrotask = (cb) => {
    if (typeof cb !== "function") throw new TypeError("queueMicrotask requires a function");
    Promise.resolve().then(cb);
  };
}"#;
/// Hands the pending request to the bundle's `call` and its output (or an `{id, ok:false, error}` line naming
/// the request's id when the bundle itself throws or rejects) back through `op_jsrt_reply`.
///
/// Unlike V8, JavaScriptCore's `Error.prototype.stack` contains only stack frames without the
/// `"ErrorName: message"` header, so `fail` prepends `String(e)` when `e.stack` does not already include it.
/// `__ENTRY__` is the bundle's global, substituted per [`Bundle`].
const PUMP: &str = r#"globalThis.__theuthPump = () => {
  const ops = Deno.core.ops;
  const entry = globalThis[__ENTRY__];
  const line = ops.op_jsrt_take_request();
  const fail = (e) => {
    let id = null;
    try {
      const r = JSON.parse(line);
      if (r && (typeof r.id === "number" || typeof r.id === "string")) id = r.id;
    } catch {}
    const head = String(e);
    const stack = e && e.stack ? String(e.stack) : "";
    const error = stack ? (stack.includes(head) ? stack : `${head}\n${stack}`) : head;
    ops.op_jsrt_reply(JSON.stringify({ id, ok: false, error }));
  };
  try {
    Promise.resolve(entry.call(line)).then((out) => ops.op_jsrt_reply(String(out)), fail);
  } catch (e) {
    fail(e);
  }
};
globalThis.__theuthVersion = () => {
  const entry = globalThis[__ENTRY__];
  const v = entry.version;
  Deno.core.ops.op_jsrt_reply(String(typeof v === "function" ? v.call(entry) : v));
};"#;

/// Property name on `globalThis` holding the non-enumerable, non-deletable private-data object that links
/// host callbacks back to this runtime's [`HostState`].
const HOST_PROP: &str = "__theuth_jsrt_host";

/// Raw C FFI bindings to Apple's `JavaScriptCore.framework` (verified against macOS & iOS SDK headers:
/// `JSBase.h`, `JSContextRef.h`, `JSObjectRef.h`, `JSStringRef.h`, `JSTypedArray.h`, `JSValueRef.h`).
#[allow(non_camel_case_types)]
mod sys {
    use std::ffi::{c_char, c_int, c_uint, c_void};

    #[repr(C)]
    pub struct OpaqueJSContextGroup {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub struct OpaqueJSContext {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub struct OpaqueJSString {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub struct OpaqueJSClass {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub struct OpaqueJSValue {
        _private: [u8; 0],
    }

    pub type JSContextGroupRef = *const OpaqueJSContextGroup;
    pub type JSContextRef = *const OpaqueJSContext;
    pub type JSGlobalContextRef = *mut OpaqueJSContext;
    pub type JSStringRef = *mut OpaqueJSString;
    pub type JSClassRef = *mut OpaqueJSClass;
    pub type JSValueRef = *const OpaqueJSValue;
    pub type JSObjectRef = *mut OpaqueJSValue;
    pub type JSPropertyAttributes = c_uint;
    pub type JSClassAttributes = c_uint;
    pub type JSChar = u16;

    pub const K_JS_PROPERTY_ATTRIBUTE_NONE: JSPropertyAttributes = 0;
    pub const K_JS_PROPERTY_ATTRIBUTE_READ_ONLY: JSPropertyAttributes = 1 << 1;
    pub const K_JS_PROPERTY_ATTRIBUTE_DONT_ENUM: JSPropertyAttributes = 1 << 2;
    pub const K_JS_PROPERTY_ATTRIBUTE_DONT_DELETE: JSPropertyAttributes = 1 << 3;

    pub type JSObjectCallAsFunctionCallback = unsafe extern "C" fn(
        ctx: JSContextRef,
        function: JSObjectRef,
        this_object: JSObjectRef,
        argument_count: usize,
        arguments: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef;

    #[repr(C)]
    pub struct JSClassDefinition {
        pub version: c_int,
        pub attributes: JSClassAttributes,
        pub class_name: *const c_char,
        pub parent_class: JSClassRef,
        pub static_values: *const c_void,
        pub static_functions: *const c_void,
        pub initialize: *const c_void,
        pub finalize: *const c_void,
        pub has_property: *const c_void,
        pub get_property: *const c_void,
        pub set_property: *const c_void,
        pub delete_property: *const c_void,
        pub get_property_names: *const c_void,
        pub call_as_function: Option<JSObjectCallAsFunctionCallback>,
        pub call_as_constructor: *const c_void,
        pub has_instance: *const c_void,
        pub convert_to_type: *const c_void,
    }

    impl JSClassDefinition {
        pub const fn empty(class_name: *const c_char) -> Self {
            Self {
                version: 0,
                attributes: 0,
                class_name,
                parent_class: std::ptr::null_mut(),
                static_values: std::ptr::null(),
                static_functions: std::ptr::null(),
                initialize: std::ptr::null(),
                finalize: std::ptr::null(),
                has_property: std::ptr::null(),
                get_property: std::ptr::null(),
                set_property: std::ptr::null(),
                delete_property: std::ptr::null(),
                get_property_names: std::ptr::null(),
                call_as_function: None,
                call_as_constructor: std::ptr::null(),
                has_instance: std::ptr::null(),
                convert_to_type: std::ptr::null(),
            }
        }
    }

    #[link(name = "JavaScriptCore", kind = "framework")]
    unsafe extern "C" {
        pub fn JSContextGroupCreate() -> JSContextGroupRef;
        pub fn JSContextGroupRelease(group: JSContextGroupRef);

        pub fn JSGlobalContextCreateInGroup(
            group: JSContextGroupRef,
            global_object_class: JSClassRef,
        ) -> JSGlobalContextRef;
        pub fn JSGlobalContextRelease(ctx: JSGlobalContextRef);
        pub fn JSContextGetGlobalObject(ctx: JSContextRef) -> JSObjectRef;

        pub fn JSStringCreateWithUTF8CString(string: *const c_char) -> JSStringRef;
        pub fn JSStringCreateWithCharacters(chars: *const JSChar, num_chars: usize) -> JSStringRef;
        pub fn JSStringRelease(string: JSStringRef);
        pub fn JSStringGetLength(string: JSStringRef) -> usize;
        pub fn JSStringGetCharactersPtr(string: JSStringRef) -> *const JSChar;

        pub fn JSEvaluateScript(
            ctx: JSContextRef,
            script: JSStringRef,
            this_object: JSObjectRef,
            source_url: JSStringRef,
            starting_line_number: c_int,
            exception: *mut JSValueRef,
        ) -> JSValueRef;

        pub fn JSValueMakeUndefined(ctx: JSContextRef) -> JSValueRef;
        pub fn JSValueMakeBoolean(ctx: JSContextRef, boolean: bool) -> JSValueRef;
        pub fn JSValueMakeString(ctx: JSContextRef, string: JSStringRef) -> JSValueRef;
        pub fn JSValueToBoolean(ctx: JSContextRef, value: JSValueRef) -> bool;
        pub fn JSValueToStringCopy(
            ctx: JSContextRef,
            value: JSValueRef,
            exception: *mut JSValueRef,
        ) -> JSStringRef;
        pub fn JSValueToObject(
            ctx: JSContextRef,
            value: JSValueRef,
            exception: *mut JSValueRef,
        ) -> JSObjectRef;
        pub fn JSValueIsObject(ctx: JSContextRef, value: JSValueRef) -> bool;
        pub fn JSValueIsString(ctx: JSContextRef, value: JSValueRef) -> bool;

        pub fn JSClassCreate(definition: *const JSClassDefinition) -> JSClassRef;
        pub fn JSClassRelease(js_class: JSClassRef);

        pub fn JSObjectMake(
            ctx: JSContextRef,
            js_class: JSClassRef,
            data: *mut c_void,
        ) -> JSObjectRef;
        pub fn JSObjectMakeError(
            ctx: JSContextRef,
            argument_count: usize,
            arguments: *const JSValueRef,
            exception: *mut JSValueRef,
        ) -> JSObjectRef;
        pub fn JSObjectMakeFunctionWithCallback(
            ctx: JSContextRef,
            name: JSStringRef,
            call_as_function: JSObjectCallAsFunctionCallback,
        ) -> JSObjectRef;
        pub fn JSObjectGetProperty(
            ctx: JSContextRef,
            object: JSObjectRef,
            property_name: JSStringRef,
            exception: *mut JSValueRef,
        ) -> JSValueRef;
        pub fn JSObjectSetProperty(
            ctx: JSContextRef,
            object: JSObjectRef,
            property_name: JSStringRef,
            value: JSValueRef,
            attributes: JSPropertyAttributes,
            exception: *mut JSValueRef,
        );
        pub fn JSObjectGetPrivate(object: JSObjectRef) -> *mut c_void;
        pub fn JSObjectSetPrivate(object: JSObjectRef, data: *mut c_void) -> bool;

        pub fn JSObjectGetTypedArrayBytesPtr(
            ctx: JSContextRef,
            object: JSObjectRef,
            exception: *mut JSValueRef,
        ) -> *mut c_void;
        pub fn JSObjectGetTypedArrayByteLength(
            ctx: JSContextRef,
            object: JSObjectRef,
            exception: *mut JSValueRef,
        ) -> usize;
        pub fn JSObjectGetTypedArrayByteOffset(
            ctx: JSContextRef,
            object: JSObjectRef,
            exception: *mut JSValueRef,
        ) -> usize;
    }
}

/// Owned `JSStringRef` that calls `JSStringRelease` on drop.
struct JsStr(sys::JSStringRef);

impl JsStr {
    fn from_str(s: &str) -> Self {
        let raw = if let Ok(c) = CString::new(s) {
            unsafe { sys::JSStringCreateWithUTF8CString(c.as_ptr()) }
        } else {
            // Preserve embedded NUL bytes losslessly via UTF-16.
            let utf16: Vec<u16> = s.encode_utf16().collect();
            unsafe { sys::JSStringCreateWithCharacters(utf16.as_ptr(), utf16.len()) }
        };
        Self(raw)
    }

    fn from_raw(raw: sys::JSStringRef) -> Self {
        Self(raw)
    }

    fn raw(&self) -> sys::JSStringRef {
        self.0
    }

    /// Convert to a Rust `String` via UTF-16 so embedded NULs are preserved and lone surrogates decode as
    /// U+FFFD (Apple's `JSStringGetUTF8CString` truncates at lone surrogates).
    fn to_rust_string(&self) -> String {
        if self.0.is_null() {
            return String::new();
        }
        unsafe {
            let len = sys::JSStringGetLength(self.0);
            if len == 0 {
                return String::new();
            }
            let ptr = sys::JSStringGetCharactersPtr(self.0);
            if ptr.is_null() {
                return String::new();
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            String::from_utf16_lossy(slice)
        }
    }
}

impl Drop for JsStr {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { sys::JSStringRelease(self.0) };
        }
    }
}

/// The request being served, its output, and a panic caught in an op — handed across the JS boundary.
#[derive(Default)]
struct Exchange {
    request: Option<String>,
    reply: Option<String>,
    crashed: Option<String>,
}

/// Per-runtime state shared between [`JsRuntime`] and its JSC host callbacks on the runtime's thread.
struct HostState {
    exchange: Exchange,
    thread_name: &'static str,
}

/// Convert a `JSValueRef` to a Rust `String`, returning an error string if stringification throws.
unsafe fn js_value_to_string(
    ctx: sys::JSContextRef,
    val: sys::JSValueRef,
) -> Result<String, String> {
    if val.is_null() {
        return Ok(String::new());
    }
    let mut exc: sys::JSValueRef = ptr::null();
    let jstr = unsafe { sys::JSValueToStringCopy(ctx, val, &mut exc) };
    let owned = JsStr::from_raw(jstr);
    if !exc.is_null() {
        return Err("failed to convert JS value to string".to_owned());
    }
    Ok(owned.to_rust_string())
}

/// Format a thrown JS exception value, appending its `.stack` frames when available.
unsafe fn format_js_exception(ctx: sys::JSContextRef, exc: sys::JSValueRef) -> String {
    if exc.is_null() {
        return "unknown JS exception".to_owned();
    }
    let msg = unsafe { js_value_to_string(ctx, exc) }
        .unwrap_or_else(|_| "unknown JS exception".to_owned());
    if unsafe { sys::JSValueIsObject(ctx, exc) } {
        let mut ignored: sys::JSValueRef = ptr::null();
        let obj = unsafe { sys::JSValueToObject(ctx, exc, &mut ignored) };
        if !obj.is_null() && ignored.is_null() {
            let stack_key = JsStr::from_str("stack");
            let stack_val =
                unsafe { sys::JSObjectGetProperty(ctx, obj, stack_key.raw(), &mut ignored) };
            if ignored.is_null()
                && !stack_val.is_null()
                && unsafe { sys::JSValueIsString(ctx, stack_val) }
                && let Ok(stack) = unsafe { js_value_to_string(ctx, stack_val) }
                && !stack.is_empty()
                && !stack.contains(&msg)
            {
                return format!("{msg}\n{stack}");
            }
        }
    }
    msg
}

/// Store a newly created JS `Error` into `*exception` (if non-null) so JSC immediately unwinds the caller.
unsafe fn set_js_error(ctx: sys::JSContextRef, exception: *mut sys::JSValueRef, message: &str) {
    if exception.is_null() {
        return;
    }
    let msg = JsStr::from_str(message);
    let arg = unsafe { sys::JSValueMakeString(ctx, msg.raw()) };
    let mut make_exc: sys::JSValueRef = ptr::null();
    let err = unsafe { sys::JSObjectMakeError(ctx, 1, &arg, &mut make_exc) };
    unsafe {
        if !err.is_null() && make_exc.is_null() {
            *exception = err.cast_const();
        } else {
            *exception = arg;
        }
    }
}

/// Look up the [`HostState`] attached to `ctx`'s global object.
unsafe fn host_state_from_ctx(ctx: sys::JSContextRef) -> Option<*mut HostState> {
    let global = unsafe { sys::JSContextGetGlobalObject(ctx) };
    if global.is_null() {
        return None;
    }
    let key = JsStr::from_str(HOST_PROP);
    let mut exc: sys::JSValueRef = ptr::null();
    let val = unsafe { sys::JSObjectGetProperty(ctx, global, key.raw(), &mut exc) };
    if !exc.is_null() || val.is_null() || !unsafe { sys::JSValueIsObject(ctx, val) } {
        return None;
    }
    let obj = unsafe { sys::JSValueToObject(ctx, val, &mut exc) };
    if !exc.is_null() || obj.is_null() {
        return None;
    }
    let ptr = unsafe { sys::JSObjectGetPrivate(obj) };
    if ptr.is_null() {
        None
    } else {
        Some(ptr.cast::<HostState>())
    }
}

/// Run a host op callback; any Rust panic is caught before crossing the `extern "C"` boundary, recorded in
/// [`Exchange::crashed`], and raised as a JS exception so the running script aborts immediately.
unsafe fn guarded_op(
    ctx: sys::JSContextRef,
    exception: *mut sys::JSValueRef,
    f: impl FnOnce(&mut HostState) -> Result<sys::JSValueRef, String>,
) -> sys::JSValueRef {
    let Some(host_ptr) = (unsafe { host_state_from_ctx(ctx) }) else {
        return unsafe { sys::JSValueMakeUndefined(ctx) };
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `host_ptr` belongs to the active `JsRuntime` on this thread, and no Rust `&mut HostState`
        // exists while JSC is executing `JSEvaluateScript`.
        let host = unsafe { &mut *host_ptr };
        f(host)
    }));
    match result {
        Ok(Ok(val)) => val,
        Ok(Err(msg)) => {
            unsafe { set_js_error(ctx, exception, &msg) };
            unsafe { sys::JSValueMakeUndefined(ctx) }
        }
        Err(payload) => {
            let message = crate::panic_message(&*payload);
            let host = unsafe { &mut *host_ptr };
            let thread = host.thread_name;
            let full = format!("panic in a {thread} op: {message}");
            host.exchange.crashed.get_or_insert_with(|| full.clone());
            unsafe { set_js_error(ctx, exception, &full) };
            unsafe { sys::JSValueMakeUndefined(ctx) }
        }
    }
}

unsafe extern "C" fn cb_take_request(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this_object: sys::JSObjectRef,
    _argument_count: usize,
    _arguments: *const sys::JSValueRef,
    exception: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        guarded_op(ctx, exception, |host| {
            let req = host.exchange.request.take().unwrap_or_default();
            let s = JsStr::from_str(&req);
            Ok(sys::JSValueMakeString(ctx, s.raw()))
        })
    }
}

unsafe extern "C" fn cb_reply(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this_object: sys::JSObjectRef,
    argument_count: usize,
    arguments: *const sys::JSValueRef,
    exception: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        guarded_op(ctx, exception, |host| {
            let out = if argument_count >= 1 && !arguments.is_null() {
                js_value_to_string(ctx, *arguments)?
            } else {
                String::new()
            };
            host.exchange.reply = Some(out);
            Ok(sys::JSValueMakeUndefined(ctx))
        })
    }
}

unsafe extern "C" fn cb_random_fill(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this_object: sys::JSObjectRef,
    argument_count: usize,
    arguments: *const sys::JSValueRef,
    exception: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        guarded_op(ctx, exception, |_host| {
            if argument_count < 1 || arguments.is_null() {
                return Err("op_jsrt_random_fill: missing TypedArray argument".to_owned());
            }
            {
                let arg0 = *arguments;
                if arg0.is_null() || !sys::JSValueIsObject(ctx, arg0) {
                    return Err("op_jsrt_random_fill: expected a TypedArray".to_owned());
                }
                let mut exc: sys::JSValueRef = ptr::null();
                let obj = sys::JSValueToObject(ctx, arg0, &mut exc);
                if !exc.is_null() || obj.is_null() {
                    return Err(format_js_exception(ctx, exc));
                }
                let byte_len = sys::JSObjectGetTypedArrayByteLength(ctx, obj, &mut exc);
                if !exc.is_null() {
                    return Err(format_js_exception(ctx, exc));
                }
                if byte_len == 0 {
                    return Ok(sys::JSValueMakeUndefined(ctx));
                }
                let byte_offset = sys::JSObjectGetTypedArrayByteOffset(ctx, obj, &mut exc);
                if !exc.is_null() {
                    return Err(format_js_exception(ctx, exc));
                }
                // Fetch `base_ptr` last: JSC's pointer is temporary across JSC API calls, and on Apple's
                // JavaScriptCore it points to the start of the backing ArrayBuffer (so `byte_offset` must be
                // added explicitly for subarray views).
                let base_ptr = sys::JSObjectGetTypedArrayBytesPtr(ctx, obj, &mut exc);
                if !exc.is_null() {
                    return Err(format_js_exception(ctx, exc));
                }
                if base_ptr.is_null() {
                    return Err("op_jsrt_random_fill: expected a TypedArray".to_owned());
                }
                let slice = std::slice::from_raw_parts_mut(
                    base_ptr.cast::<u8>().add(byte_offset),
                    byte_len,
                );
                getrandom::getrandom(slice).expect("OS randomness");
                Ok(sys::JSValueMakeUndefined(ctx))
            }
        })
    }
}

/// Test hook for the panic guard: registered only in this crate's unit tests.
#[cfg(test)]
unsafe extern "C" fn cb_test_panic(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this_object: sys::JSObjectRef,
    _argument_count: usize,
    _arguments: *const sys::JSValueRef,
    exception: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        guarded_op(ctx, exception, |_host| {
            panic!("test: panic inside a js-runtime op")
        })
    }
}

unsafe extern "C" fn cb_print(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this_object: sys::JSObjectRef,
    argument_count: usize,
    arguments: *const sys::JSValueRef,
    exception: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        guarded_op(ctx, exception, |_host| {
            if argument_count >= 1 && !arguments.is_null() {
                let msg = js_value_to_string(ctx, *arguments)?;
                let is_err =
                    argument_count >= 2 && sys::JSValueToBoolean(ctx, *arguments.add(1));
                if is_err {
                    eprint!("{msg}");
                } else {
                    print!("{msg}");
                }
            }
            Ok(sys::JSValueMakeUndefined(ctx))
        })
    }
}

unsafe extern "C" fn cb_create_timer(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this_object: sys::JSObjectRef,
    _argument_count: usize,
    _arguments: *const sys::JSValueRef,
    _exception: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    // Timers are only referenced at bundle load time and never scheduled on the synchronous/microtask
    // bundle paths; return a truthy handle so `polyfill.js`'s `clearTimeout`/`clearInterval` work cleanly.
    unsafe { sys::JSValueMakeBoolean(ctx, true) }
}

unsafe extern "C" fn cb_cancel_timer(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this_object: sys::JSObjectRef,
    _argument_count: usize,
    _arguments: *const sys::JSValueRef,
    _exception: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe { sys::JSValueMakeUndefined(ctx) }
}

/// Set a property on a JSC object, returning `Err(Error::Crashed)` if JSC throws.
unsafe fn set_prop(
    ctx: sys::JSContextRef,
    obj: sys::JSObjectRef,
    name: &str,
    value: sys::JSValueRef,
    attrs: sys::JSPropertyAttributes,
) -> Result<(), Error> {
    let key = JsStr::from_str(name);
    let mut exc: sys::JSValueRef = ptr::null();
    unsafe { sys::JSObjectSetProperty(ctx, obj, key.raw(), value, attrs, &mut exc) };
    if !exc.is_null() {
        return Err(Error::Crashed(format!(
            "install property {name}: {}",
            unsafe { format_js_exception(ctx, exc) }
        )));
    }
    Ok(())
}

/// Create a JS function backed by a C callback and install it as `name` on `obj`.
unsafe fn set_callback(
    ctx: sys::JSContextRef,
    obj: sys::JSObjectRef,
    name: &str,
    cb: sys::JSObjectCallAsFunctionCallback,
) -> Result<(), Error> {
    let fn_name = JsStr::from_str(name);
    let func = unsafe { sys::JSObjectMakeFunctionWithCallback(ctx, fn_name.raw(), cb) };
    if func.is_null() {
        return Err(Error::Crashed(format!("create JSC callback {name}")));
    }
    unsafe {
        set_prop(
            ctx,
            obj,
            name,
            func.cast_const(),
            sys::K_JS_PROPERTY_ATTRIBUTE_NONE,
        )
    }
}

/// No-op on Apple JavaScriptCore: JSC needs no process-wide platform initialisation or main-thread throwaway
/// isolate. Kept idempotent and safe in any context to match the desktop `deno_core` API.
pub fn init_platform() {
    first_isolate();
}

/// [`init_platform`]'s body (no-op on JSC; kept so unit tests compile identically across backends).
pub(crate) fn first_isolate() {}

/// One in-process bundle: an isolated Apple JavaScriptCore context group and global context with the
/// polyfill(s) and the bundle loaded. `!Send`: create and drop it on the same thread (or use
/// [`crate::JsThread`]).
///
/// After [`Error::Timeout`], [`Error::OutOfMemory`] or [`Error::Crashed`] the runtime refuses further work
/// ([`Self::failure`]); drop it and build a new one.
pub struct JsRuntime {
    group: sys::JSContextGroupRef,
    ctx: sys::JSGlobalContextRef,
    host_class: sys::JSClassRef,
    host_obj: sys::JSObjectRef,
    /// Owned heap allocation freed in [`Drop`] strictly after `JSGlobalContextRelease` and
    /// `JSContextGroupRelease` (raw pointer so moving `JsRuntime` never invalidates the pointer stored in
    /// `host_obj`'s private data).
    host: *mut HostState,
    bundle: &'static Bundle,
    limits: Limits,
    failure: Option<Error>,
    /// A request has been served on this runtime: later ones get [`Limits::deadline`], the first
    /// [`Limits::first_deadline`].
    warm: bool,
}

impl Drop for JsRuntime {
    fn drop(&mut self) {
        unsafe {
            // Detach the Rust pointer before releasing the context so teardown can never dereference it.
            sys::JSObjectSetPrivate(self.host_obj, ptr::null_mut());
            sys::JSGlobalContextRelease(self.ctx);
            sys::JSContextGroupRelease(self.group);
            sys::JSClassRelease(self.host_class);
            drop(Box::from_raw(self.host));
        }
    }
}

impl JsRuntime {
    /// Build a fresh JavaScriptCore context group + global context, install the host ops, and load the
    /// polyfill(s) and `bundle` under at least [`LOAD_DEADLINE`].
    pub fn new(bundle: &'static Bundle, limits: Limits) -> Result<Self, Error> {
        let group = unsafe { sys::JSContextGroupCreate() };
        if group.is_null() {
            return Err(Error::Crashed("JSContextGroupCreate returned null".into()));
        }
        let ctx = unsafe { sys::JSGlobalContextCreateInGroup(group, ptr::null_mut()) };
        if ctx.is_null() {
            unsafe { sys::JSContextGroupRelease(group) };
            return Err(Error::Crashed(
                "JSGlobalContextCreateInGroup returned null".into(),
            ));
        }
        let def = sys::JSClassDefinition::empty(c"TheuthJsHost".as_ptr());
        let host_class = unsafe { sys::JSClassCreate(&def) };
        if host_class.is_null() {
            unsafe {
                sys::JSGlobalContextRelease(ctx);
                sys::JSContextGroupRelease(group);
            }
            return Err(Error::Crashed("JSClassCreate returned null".into()));
        }
        let host = Box::into_raw(Box::new(HostState {
            exchange: Exchange::default(),
            thread_name: bundle.thread,
        }));
        let host_obj = unsafe { sys::JSObjectMake(ctx.cast_const(), host_class, host.cast()) };
        if host_obj.is_null() {
            unsafe {
                sys::JSGlobalContextRelease(ctx);
                sys::JSContextGroupRelease(group);
                sys::JSClassRelease(host_class);
                drop(Box::from_raw(host));
            }
            return Err(Error::Crashed(
                "JSObjectMake for host state returned null".into(),
            ));
        }

        // Loading the bundle is construction, not a request: it runs under at least LOAD_DEADLINE.
        let deadline = limits.deadline;
        let mut this = Self {
            group,
            ctx,
            host_class,
            host_obj,
            host,
            bundle,
            limits: Limits {
                deadline: deadline.max(LOAD_DEADLINE),
                ..limits
            },
            failure: None,
            warm: false,
        };
        // From here on, any `?` return drops `this` cleanly via `impl Drop for JsRuntime`.
        this.install_host_globals()?;
        this.run("<theuth:jsc-bootstrap>", BOOTSTRAP, false)?;
        this.run("<theuth:polyfill>", POLYFILL, false)?;
        if bundle.web_polyfill {
            this.run("<theuth:polyfill-web>", POLYFILL_WEB, false)?;
        }
        this.run(bundle.name, bundle.source, false)?;
        let entry = serde_json::to_string(bundle.global)
            .map_err(|e| Error::Crashed(format!("bundle global: {e}")))?;
        let pump = PUMP.replace("__ENTRY__", &entry);
        this.run("<theuth:pump>", &pump, false)?;
        this.limits.deadline = deadline;
        Ok(this)
    }

    /// Install `__theuth_jsrt_host` and `Deno.core.{ops, print, createTimer, cancelTimer}` on `globalThis`
    /// so `polyfill.js`, `polyfill-web.js`, and `PUMP` run verbatim.
    fn install_host_globals(&mut self) -> Result<(), Error> {
        let ctx = self.ctx.cast_const();
        unsafe {
            let global = sys::JSContextGetGlobalObject(ctx);
            if global.is_null() {
                return Err(Error::Crashed(
                    "JSContextGetGlobalObject returned null".into(),
                ));
            }
            set_prop(
                ctx,
                global,
                HOST_PROP,
                self.host_obj.cast_const(),
                sys::K_JS_PROPERTY_ATTRIBUTE_READ_ONLY
                    | sys::K_JS_PROPERTY_ATTRIBUTE_DONT_ENUM
                    | sys::K_JS_PROPERTY_ATTRIBUTE_DONT_DELETE,
            )?;

            let ops = sys::JSObjectMake(ctx, ptr::null_mut(), ptr::null_mut());
            set_callback(ctx, ops, "op_jsrt_take_request", cb_take_request)?;
            set_callback(ctx, ops, "op_jsrt_reply", cb_reply)?;
            set_callback(ctx, ops, "op_jsrt_random_fill", cb_random_fill)?;
            #[cfg(test)]
            set_callback(ctx, ops, "op_jsrt_test_panic", cb_test_panic)?;

            let core = sys::JSObjectMake(ctx, ptr::null_mut(), ptr::null_mut());
            set_prop(
                ctx,
                core,
                "ops",
                ops.cast_const(),
                sys::K_JS_PROPERTY_ATTRIBUTE_NONE,
            )?;
            set_callback(ctx, core, "print", cb_print)?;
            set_callback(ctx, core, "createTimer", cb_create_timer)?;
            set_callback(ctx, core, "cancelTimer", cb_cancel_timer)?;

            let deno = sys::JSObjectMake(ctx, ptr::null_mut(), ptr::null_mut());
            set_prop(
                ctx,
                deno,
                "core",
                core.cast_const(),
                sys::K_JS_PROPERTY_ATTRIBUTE_NONE,
            )?;
            set_prop(
                ctx,
                global,
                "Deno",
                deno.cast_const(),
                sys::K_JS_PROPERTY_ATTRIBUTE_NONE,
            )?;
        }
        Ok(())
    }

    /// The bundle this runtime serves.
    pub fn bundle(&self) -> &'static Bundle {
        self.bundle
    }

    /// The guards this runtime enforces.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The fatal error that stopped this runtime, if any: it must be dropped and rebuilt.
    pub fn failure(&self) -> Option<&Error> {
        self.failure.as_ref()
    }

    /// One request line → the bundle's output lines for it: event lines first, the reply last.
    /// A JS exception is an `ok:false` reply line, not an `Err`.
    pub fn call(&mut self, line: &str) -> Result<Vec<String>, Error> {
        self.serve(line.to_owned())
    }

    /// [`Self::call`] taking the line by value (no copy).
    pub(crate) fn serve(&mut self, line: String) -> Result<Vec<String>, Error> {
        if line.len() > self.limits.max_line {
            return Err(Error::TooLarge {
                len: line.len(),
                max: self.limits.max_line,
            });
        }
        self.exchange(|x| x.request = Some(line));
        let warm_deadline = self.limits.deadline;
        if !self.warm {
            self.limits.deadline = self.limits.first_deadline.max(warm_deadline);
            self.warm = true;
        }
        let out = self.run("<theuth:call>", "globalThis.__theuthPump()", true);
        self.limits.deadline = warm_deadline;
        // The slot is per request: a pump that never took its line must not hand it to the next one.
        self.exchange(|x| x.request = None);
        let Some(out) = out? else {
            // Microtask queue drained with no reply: the request's promise is orphaned, so this runtime is done.
            let e = Error::Crashed("the request ended without its reply".into());
            self.failure = Some(e.clone());
            return Err(e);
        };
        Ok(out.split('\n').map(str::to_owned).collect())
    }

    /// The bundle's version string (its global's `version`, called when it is a function).
    pub fn version(&mut self) -> Result<String, Error> {
        self.run("<theuth:version>", "globalThis.__theuthVersion()", true)?
            .ok_or_else(|| Error::Js("no version".into()))
    }

    /// Run a raw classic script under the same guards as a request (containment tests).
    #[doc(hidden)]
    pub fn eval_raw(&mut self, script: &str) -> Result<(), Error> {
        self.run("<theuth:raw>", script, false).map(|_| ())
    }

    fn exchange<R>(&mut self, f: impl FnOnce(&mut Exchange) -> R) -> R {
        // SAFETY: `self.host` is uniquely owned by `self` and JSC is not currently executing.
        f(unsafe { &mut (*self.host).exchange })
    }

    /// Evaluate `source` in JSC (which drains the Promise microtask queue before returning to C), then check
    /// op panics and wall-clock deadline (precedence: op panic > deadline > script error).
    fn run(
        &mut self,
        name: &'static str,
        source: &str,
        until_reply: bool,
    ) -> Result<Option<String>, Error> {
        if let Some(e) = &self.failure {
            return Err(Error::Crashed(format!(
                "runtime unusable after an earlier failure: {e}"
            )));
        }
        self.exchange(|x| x.reply = None);
        let started = Instant::now();
        let eval_result = self.eval_script(name, source);
        let elapsed = started.elapsed();

        if let Some(m) = self.exchange(|x| x.crashed.take()) {
            let e = Error::Crashed(m);
            self.failure = Some(e.clone());
            return Err(e);
        }
        if elapsed > self.limits.deadline {
            let e = Error::Timeout;
            self.failure = Some(e.clone());
            return Err(e);
        }
        eval_result?;
        if until_reply {
            Ok(self.exchange(|x| x.reply.take()))
        } else {
            Ok(None)
        }
    }

    fn eval_script(&mut self, name: &'static str, source: &str) -> Result<(), Error> {
        let script = JsStr::from_str(source);
        let url = JsStr::from_str(name);
        let ctx = self.ctx.cast_const();
        let mut exc: sys::JSValueRef = ptr::null();
        let val = unsafe {
            sys::JSEvaluateScript(ctx, script.raw(), ptr::null_mut(), url.raw(), 1, &mut exc)
        };
        if !exc.is_null() {
            return Err(Error::Js(unsafe { format_js_exception(ctx, exc) }));
        }
        if val.is_null() {
            return Err(Error::Js("JSEvaluateScript returned null".into()));
        }
        Ok(())
    }
}
