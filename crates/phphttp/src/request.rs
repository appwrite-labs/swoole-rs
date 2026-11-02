//! `Swoole\Http\Request`.
use crate::{
    context::{self, Holder},
    host::{
        self, COMPLETED, Context, HTTP_CONTEXT_UNAVAILABLE, HTTP2, swr_http_host_request_create,
        swr_http_host_request_free,
    },
    parse::{self, body, method_name},
    state::State,
};
use std::ptr;
use swoole_rs_phpstubs::http_request;
use swoole_rs_zend::{
    Call, Class, declare_long, declare_null, handlers,
    sys::{IS_STRING, ZEND_ACC_PUBLIC, zend_class_entry, zend_object},
    type_of,
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
        // SAFETY: the context the object holds; C++ releases it.
        unsafe { swr_http_host_request_free(ctx) };
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

fn create_request(call: &Call) {
    let Ok(options) = call.arity(0, 1).and_then(|()| call.array_opt(1)) else {
        return call.set_bool(false);
    };
    // SAFETY: the return slot and an optional array argument of this call.
    let bailed = unsafe {
        swr_http_host_request_create(call.return_value(), options.unwrap_or(ptr::null_mut()))
    };
    host::raise(bailed);
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
