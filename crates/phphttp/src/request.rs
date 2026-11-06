//! `Swoole\Http\Request`.
use crate::{
    context::{self, Holder},
    host::{self, COMPLETED, Context, HTTP_CONTEXT_UNAVAILABLE, HTTP2},
    parse::{self, CAPABILITIES, Property, body, method_name},
    send,
    state::State,
};
use std::{ffi::c_void, sync::atomic::Ordering::Relaxed};
use swoole_rs_httpbindings::{Text, swr_v1_httpbindings_request_option};
use swoole_rs_phpstubs::http_request;
use swoole_rs_zend::{
    Call, Class, declare_long, declare_null, handlers, init_object, string,
    sys::{
        IS_STRING, ZEND_ACC_PUBLIC, swr_zend_hash_foreach_kv, swr_zend_string_release,
        swr_zval_is_true, zend_class_entry, zend_object, zend_string, zend_ulong, zval,
    },
    type_of,
    value::{self, get_long, get_string, obj_of},
};
use swoole_rs_zend::{method_table, sys as z};

static REQUEST: Class = Class::new();

/// The registered class entry.
pub(crate) fn entry() -> *mut zend_class_entry {
    REQUEST.entry()
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass of it.
    unsafe { context::create(&REQUEST, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    let ctx = unsafe { *Holder::data(std) };
    if !ctx.is_null() {
        let state = send::state(ctx);
        // SAFETY: the request's own `tmpfiles` slot (null when it has none).
        unsafe { z::swr_zend_http_uploads_release(state.request.ztmpfiles.ptr) };
        state.request.zobject.ptr = std::ptr::null_mut();
        // SAFETY: the context the object held.
        unsafe { context::swr_v1_phphttp_context_free(ctx) };
    }
    // SAFETY: as above.
    unsafe { Holder::dtor(std) };
}

method_table! {
    static METHODS = [
        c"getContent" => zim_swoole_http_request_getContent, &http_request::arginfo_class_Swoole_Http_Request_getContent, z::ZEND_ACC_PUBLIC;
        c"rawContent" => zim_swoole_http_request_getContent, &http_request::arginfo_class_Swoole_Http_Request_getContent, z::ZEND_ACC_PUBLIC;
        c"getData" => zim_swoole_http_request_getData, &http_request::arginfo_class_Swoole_Http_Request_getData, z::ZEND_ACC_PUBLIC;
        c"create" => zim_swoole_http_request_create, &http_request::arginfo_class_Swoole_Http_Request_create, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"parse" => zim_swoole_http_request_parse, &http_request::arginfo_class_Swoole_Http_Request_parse, z::ZEND_ACC_PUBLIC;
        c"isCompleted" => zim_swoole_http_request_isCompleted, &http_request::arginfo_class_Swoole_Http_Request_isCompleted, z::ZEND_ACC_PUBLIC;
        c"getMethod" => zim_swoole_http_request_getMethod, &http_request::arginfo_class_Swoole_Http_Request_getMethod, z::ZEND_ACC_PUBLIC;
    ];
}

/// MINIT: register the class as `php_swoole_http_request_minit` did.
/// # Safety
/// Called once, at MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_request_minit() -> *mut zend_class_entry {
    // SAFETY: MINIT, with a static table.
    unsafe {
        REQUEST.register(
            c"Swoole\\Http\\Request",
            METHODS.as_ptr(),
            |entry, handlers| {
                context::configure(entry, handlers, create, free);
            },
        );
    }
    let ce = REQUEST.entry();
    for name in [c"fd", c"streamId"] {
        // SAFETY: MINIT, with the class just registered.
        unsafe { declare_long(ce, name, 0, ZEND_ACC_PUBLIC) };
    }
    for name in [
        c"header",
        c"server",
        c"cookie",
        c"get",
        c"files",
        c"post",
        c"tmpfiles",
    ] {
        // SAFETY: as above.
        unsafe { declare_null(ce, name) };
    }
    ce
}

/// `http_request_get_and_check_context`.
fn checked(call: &Call) -> Option<*mut Context> {
    let ctx = context::this(call);
    if ctx.is_null() {
        host::set_last_error(HTTP_CONTEXT_UNAVAILABLE);
        call.set_bool(false);
        return None;
    }
    Some(ctx)
}

fn get_content(call: &Call) {
    let Some(ctx) = checked(call) else { return };
    match body(ctx.cast()) {
        Some(bytes) => call.set_bytes(bytes),
        None => call.set_empty_string(),
    }
}

fn get_data(call: &Call) {
    let Some(ctx) = checked(call) else { return };
    if host::flags(ctx) & HTTP2 != 0 {
        host::fatal("unable to get data from HTTP2 request");
        return call.set_bool(false);
    }
    // SAFETY: a live context; only the field's address is taken.
    let data = unsafe { &raw mut (*ctx.cast::<State>()).request.zdata };
    // SAFETY: the context's request data is a live zval.
    if unsafe { type_of(data) } == IS_STRING {
        // SAFETY: as above.
        return unsafe { call.set_copy(data) };
    }
    call.set_empty_string();
}

/// `SW_Z_BEST_SPEED`.
const BEST_SPEED: i8 = 1;

/// `Request::create(array $options = [])`: a request parsed by hand.
fn create_request(call: &Call) {
    let Ok(options) = call.arity(0, 1).and_then(|()| call.array_opt(1)) else {
        return call.set_bool(false);
    };
    let ctx = context::swr_v1_phphttp_context_new();
    parse::init_request_parser(ctx);
    let object = call.return_value();
    // SAFETY: the return slot and the registered class.
    unsafe { init_object(object, entry()) };
    let state = send::state(ctx);
    // SAFETY: the object just made; the context borrows it, as upstream's does.
    state.request.zobject.value = unsafe { *object };
    state.request.zobject.ptr = &raw mut state.request.zobject.value;
    // SAFETY: the object just made.
    let std = unsafe { obj_of(object) };
    // SAFETY: one of our objects.
    unsafe { context::swr_v1_phphttp_set_context(std, ctx) };
    state.parse_cookie = 1;
    state.parse_body = 1;
    state.parse_files = 1;
    let compression = CAPABILITIES.load(Relaxed) & 8 != 0;
    if compression {
        state.enable_compression = 1;
        state.compression_level = BEST_SPEED;
    }
    if let Some(options) = options {
        // SAFETY: an array argument of this call.
        let table = unsafe { value::table(options) };
        // SAFETY: a live array; `option` takes the context.
        unsafe { swr_zend_hash_foreach_kv(table, Some(option), ctx.cast()) };
    }
    parse::property(send::state(ctx), Property::Server);
    parse::property(send::state(ctx), Property::Header);
}

/// One of `Request::create`'s options (a bailout in a conversion is deferred).
unsafe extern "C" fn option(
    key: *mut zend_string,
    _: zend_ulong,
    zvalue: *mut zval,
    context: *mut c_void,
) {
    if key.is_null() {
        return;
    }
    // SAFETY: the array's string key.
    let name = unsafe { string::bytes(key) };
    let capabilities = CAPABILITIES.load(Relaxed);
    let wanted = u32::from(capabilities & 8 != 0) | u32::from(capabilities & 1 != 0) << 1;
    let text = Text {
        data: name.as_ptr(),
        length: name.len(),
    };
    // SAFETY: the key's bytes, borrowed for the call.
    let option = unsafe { swr_v1_httpbindings_request_option(text, wanted) };
    let state = send::state(context);
    // SAFETY: the array's live value.
    let truth = || u8::from(unsafe { swr_zval_is_true(zvalue) });
    match option {
        1 => state.parse_cookie = truth(),
        2 => state.parse_body = truth(),
        3 => state.parse_files = truth(),
        4 => state.enable_compression = truth(),
        5 => {
            // SAFETY: as above.
            let level = unsafe { get_long(zvalue) };
            #[expect(
                clippy::cast_possible_truncation,
                reason = "upstream's int8_t compression_level"
            )]
            let level = level as i8;
            send::state(context).compression_level = level;
        }
        6 => state.websocket_compression = truth(),
        7 => {
            // SAFETY: as above.
            let dir = unsafe { get_string(zvalue) };
            if dir.is_null() {
                return;
            }
            // SAFETY: the converted string.
            let bytes = unsafe { string::bytes(dir) };
            send::state(context).upload_tmp_dir = Some(crate::context::directory(bytes));
            // SAFETY: the reference `get_string` returned.
            unsafe { swr_zend_string_release(dir) };
        }
        _ => {}
    }
}

fn parse(call: &Call) {
    let ctx = context::this(call);
    if ctx.is_null() {
        host::set_last_error(HTTP_CONTEXT_UNAVAILABLE);
        return call.set_bool(false);
    }
    if host::flags(ctx) & COMPLETED != 0 {
        return call.set_bool(false);
    }
    let Ok(Some(data)) = call.arity(1, 1).and_then(|()| call.string_opt(1)) else {
        return call.set_bool(false);
    };
    let parsed = parse::parse_piece(ctx.cast(), data);
    call.set_long(i64::try_from(parsed).unwrap_or(i64::MAX));
}

fn get_method(call: &Call) {
    let Some(ctx) = checked(call) else { return };
    if host::flags(ctx) & HTTP2 != 0 {
        // SAFETY: a live context; its `server` array is live while the request is.
        let server = unsafe { (*ctx.cast::<State>()).request.zserver.ptr };
        let method = parse::find(server, b"request_method");
        if method.is_null() {
            return call.set_null();
        }
        // SAFETY: a live zval from that array.
        return unsafe { call.set_copy(method) };
    }
    call.set_bytes(method_name(ctx.cast()).to_bytes());
}

fn is_completed(call: &Call) {
    let Some(ctx) = checked(call) else { return };
    call.set_bool(host::flags(ctx) & COMPLETED != 0);
}

handlers! {
    zim_swoole_http_request_getContent => get_content;
    zim_swoole_http_request_getData => get_data;
    zim_swoole_http_request_create => create_request;
    zim_swoole_http_request_parse => parse;
    zim_swoole_http_request_getMethod => get_method;
    zim_swoole_http_request_isCompleted => is_completed;
}
