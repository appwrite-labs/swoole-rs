//! `Swoole\Http\Request`.
use crate::{
    context::{self, Ctx, Holder},
    host::{self, COMPLETED, HTTP_CONTEXT_UNAVAILABLE, HTTP2},
    parse::{self, CAPABILITIES, Property, body, method_name},
    state::State,
};
use std::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::CCast;
use swoole_rs_httpbindings::request_option;
use swoole_rs_phpstubs::http_request;
use swoole_rs_phpswoole::MinitArgs;
use swoole_rs_zend::{
    Call, Custom, CustomClass, Key, Object, Value, handlers, sys::ZEND_ACC_PUBLIC,
};
use swoole_rs_zend::{method_table, sys as z};

pub(crate) static REQUEST: CustomClass<Data> = CustomClass::new();

/// A request object's context.
pub(crate) struct Data(pub(crate) Holder);

impl Custom for Data {
    fn class() -> &'static CustomClass<Self> {
        &REQUEST
    }

    fn new() -> Self {
        Self(Holder::new(None))
    }
}

/// The object goes: its uploads are removed and the context, unless the
/// response still holds it, freed.
impl Drop for Data {
    fn drop(&mut self) {
        let Some(mut ctx) = self.0.get() else { return };
        let state = ctx.state();
        if state.request.ztmpfiles.get().is_some() {
            let std = Object::std(std::ptr::from_mut(self).cast::<Object<Self>>());
            // SAFETY: the request object, its properties still live, and the
            // declared `tmpfiles` slot's offset.
            unsafe { z::swr_zend_http_uploads_release(std, parse::tmpfiles_offset()) };
        }
        state.request.zobject.clear();
        // SAFETY: the context the object held.
        unsafe { context::free(ctx) };
    }
}

/// The method's `$this` context.
fn this(call: &Call) -> Option<Ctx> {
    call.custom::<Data>().and_then(|data| data.0.get())
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
pub fn request_minit(args: &MinitArgs) {
    let minit = args.minit;
    minit.register_object::<Data>(c"Swoole\\Http\\Request", METHODS.table(), 0);
    for name in [c"fd", c"streamId"] {
        minit.property_long(&REQUEST, name, 0, ZEND_ACC_PUBLIC);
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
        minit.property_null(&REQUEST, name, ZEND_ACC_PUBLIC);
    }
}

/// `http_request_get_and_check_context`.
fn checked(call: &Call) -> Option<Ctx> {
    let ctx = this(call);
    if ctx.is_none() {
        host::set_last_error(HTTP_CONTEXT_UNAVAILABLE);
        call.set_bool(false);
    }
    ctx
}

fn get_content(call: &Call) {
    let Some(mut ctx) = checked(call) else { return };
    match body(ctx.state()) {
        Some(bytes) => call.set_bytes(bytes),
        None => call.set_empty_string(),
    }
}

fn get_data(call: &Call) {
    let Some(mut ctx) = checked(call) else { return };
    if ctx.flags() & HTTP2 != 0 {
        host::fatal("unable to get data from HTTP2 request");
        return call.set_bool(false);
    }
    let data = &ctx.state().request.zdata;
    if data.str().is_some() {
        return call.set_owned(data.copy());
    }
    call.set_empty_string();
}

/// `SW_Z_BEST_SPEED`.
const BEST_SPEED: i8 = 1;

/// `Request::create(array $options = [])`: a request parsed by hand.
fn create_request(call: &Call) {
    let options = call.arity(0, 1).and_then(|()| {
        (call.num_args() >= 1)
            .then(|| call.array_ref(1))
            .transpose()
    });
    let Ok(options) = options else {
        return call.set_bool(false);
    };
    let mut ctx = context::alloc();
    parse::init_request_parser(ctx);
    let held = ctx;
    let state = ctx.state();
    let slot = &mut state.request.zobject;
    let _made = context::new_object(slot, context::Kind::Request, held);
    // SAFETY: the object just made. The return slot takes the reference; the
    // context borrows it, as upstream's does.
    unsafe { call.set_value(slot.copy_value()) };
    state.parse_cookie = true;
    state.parse_body = true;
    state.parse_files = true;
    let compression = CAPABILITIES.load(Relaxed) & 8 != 0;
    if compression {
        state.enable_compression = true;
        state.compression_level = BEST_SPEED;
    }
    if let Some(options) = options {
        options.for_each(|key, value| {
            if let Key::Name(name) = key {
                option(ctx.state(), name, value);
            }
        });
    }
    parse::property(ctx.state(), Property::Server);
    parse::property(ctx.state(), Property::Header);
}

/// One of `Request::create`'s options (a bailout in a conversion is deferred).
fn option(state: &mut State, name: &[u8], value: &Value) {
    let capabilities = CAPABILITIES.load(Relaxed);
    let wanted = u32::from(capabilities & 8 != 0) | (u32::from(capabilities & 1 != 0) << 1);
    let option = request_option(name, wanted);
    let truth = || value.truthy();
    match option {
        1 => state.parse_cookie = truth(),
        2 => state.parse_body = truth(),
        3 => state.parse_files = truth(),
        4 => state.enable_compression = truth(),
        5 => {
            let level = value.long().c_cast::<i8>();
            state.compression_level = level;
        }
        6 => state.websocket_compression = truth(),
        7 => state.upload_tmp_dir = Some(crate::context::directory(value.string().bytes())),
        _ => {}
    }
}

fn parse(call: &Call) {
    let Some(mut ctx) = checked(call) else { return };
    if ctx.flags() & COMPLETED != 0 {
        return call.set_bool(false);
    }
    let Ok(Some(data)) = call.arity(1, 1).and_then(|()| call.string_opt(1)) else {
        return call.set_bool(false);
    };
    let parsed = parse::parse_piece(ctx.state(), data);
    call.set_long(i64::try_from(parsed).unwrap_or(i64::MAX));
}

fn get_method(call: &Call) {
    let Some(mut ctx) = checked(call) else { return };
    if ctx.flags() & HTTP2 != 0 {
        let method = ctx
            .state()
            .request
            .zserver
            .get()
            .and_then(Value::array)
            .and_then(|server| server.find(b"request_method"));
        return match method {
            Some(method) => call.set_owned(method.copy()),
            None => call.set_null(),
        };
    }
    call.set_bytes(method_name(ctx.state()).as_bytes());
}

fn is_completed(call: &Call) {
    let Some(ctx) = checked(call) else { return };
    call.set_bool(ctx.flags() & COMPLETED != 0);
}

handlers! {
    zim_swoole_http_request_getContent => get_content;
    zim_swoole_http_request_getData => get_data;
    zim_swoole_http_request_create => create_request;
    zim_swoole_http_request_parse => parse;
    zim_swoole_http_request_getMethod => get_method;
    zim_swoole_http_request_isCompleted => is_completed;
}
