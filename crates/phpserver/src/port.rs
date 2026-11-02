//! `Swoole\Server\Port`: registration, objects and every method. C++ keeps the
//! `ListenPort` and Server these configure and reaches them for Rust through
//! the host table (`native/ports/swoole_rs_phpserver.h`).
use std::{
    ffi::{c_char, c_int, c_void},
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_ports::{
    swr_v1_ports_callback_hook, swr_v1_ports_callback_property, swr_v1_ports_class_property,
    swr_v1_ports_event,
};
use swoole_rs_zend::{
    Call, Class, Object,
    string::bytes,
    sys::{
        _zend_bailout, IS_NULL, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PRIVATE, ZEND_ACC_PUBLIC,
        swr_zend_string_release, swr_zval_copy, swr_zval_get_string, zend_array, zend_class_entry,
        zend_declare_property_bool, zend_declare_property_long, zend_declare_property_null,
        zend_function_entry, zend_object, zend_object_handlers, zend_read_property,
        zend_update_property, zval,
    },
    throw_error, type_of, unset_property_deny,
};

/// `PHP_SWOOLE_SERVER_PORT_CALLBACK_NUM`.
const CALLBACKS: usize = 12;

/// `swoole::ServerPortProperty`: pointers only, so Rust lays it out as C++ does.
#[repr(C)]
pub struct Property {
    callbacks: [*mut c_void; CALLBACKS],
    serv: *mut c_void,
    port: *mut c_void,
    zsetting: *mut zval,
}

/// `ServerPortObject` without its `zend_object`: the `ListenPort` and property.
#[repr(C)]
pub struct Data {
    port: *mut c_void,
    property: Property,
}

type Port = Object<Data>;

/// `swr_ports_property_kind` (`native/ports/swoole_rs_ports.h`).
const PROPERTY_END: u32 = 0;
const PROPERTY_LONG: u32 = 2;
const PROPERTY_BOOL: u32 = 3;

/// `swr_v1_phpserver_port_host`: what the methods ask of C++.
#[repr(C)]
pub struct PortHost {
    invalid: unsafe extern "C" fn(*mut zend_object),
    deref: unsafe extern "C" fn(*mut zend_object),
    started: unsafe extern "C" fn(*mut c_void) -> bool,
    warn: unsafe extern "C" fn(c_int, *const c_char),
    callable_create: unsafe extern "C" fn(*mut zval) -> *mut c_void,
    callable_free: unsafe extern "C" fn(*mut c_void),
    hook: unsafe extern "C" fn(*mut c_void, u32),
    settings: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut zend_array) -> bool,
    finish: unsafe extern "C" fn(*mut zend_object, *mut zval, *mut c_void) -> bool,
    socket: unsafe extern "C" fn(*mut c_void, *mut zval),
}

static HOST: AtomicPtr<PortHost> = AtomicPtr::new(ptr::null_mut());
static PORT: Class = Class::new();

fn host() -> &'static PortHost {
    // SAFETY: MINIT stores C++'s static table before any object or method exists.
    unsafe { &*HOST.load(Ordering::Acquire) }
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass of it;
    // null pointers are a port not yet bound to a server.
    unsafe { Port::create(&PORT, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created; C++ drops its callables.
    unsafe { (host().deref)(std) };
    // SAFETY: as above.
    unsafe { Port::dtor(std) };
}

fn configure(
    entry: &mut zend_class_entry,
    handlers: &mut zend_object_handlers,
    not_serializable: bool,
) {
    if not_serializable {
        entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    }
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    Port::install(entry, handlers, create, free);
}

/// MINIT: register the class as `php_swoole_server_port_minit` did.
/// # Safety
/// Called once, at MINIT, with a static `PHP_FE_END`-terminated method table
/// and a host table that lives for the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_port_minit(
    methods: *const zend_function_entry,
    table: *const PortHost,
    not_serializable: bool,
) -> *mut zend_class_entry {
    HOST.store(table.cast_mut(), Ordering::Release);
    // SAFETY: MINIT, with the caller's table.
    unsafe {
        PORT.register(c"Swoole\\Server\\Port", methods, |entry, handlers| {
            configure(entry, handlers, not_serializable);
        });
    };
    let ce = PORT.entry();
    for index in 0.. {
        let property = swr_v1_ports_class_property(index);
        if property.kind == PROPERTY_END {
            break;
        }
        let access = if property.private == 0 {
            ZEND_ACC_PUBLIC
        } else {
            ZEND_ACC_PRIVATE
        };
        #[expect(
            clippy::cast_possible_wrap,
            reason = "ZEND_ACC_* visibilities are small"
        )]
        let access = access as c_int;
        let (name, length) = (property.name.data.cast::<c_char>(), property.name.len);
        match property.kind {
            // SAFETY: MINIT, with the class just registered; Zend copies the name.
            PROPERTY_LONG => unsafe {
                zend_declare_property_long(ce, name, length, property.value, access);
            },
            // SAFETY: as above.
            PROPERTY_BOOL => unsafe {
                zend_declare_property_bool(ce, name, length, property.value, access);
            },
            // SAFETY: as above.
            _ => unsafe { zend_declare_property_null(ce, name, length, access) },
        }
    }
    ce
}

fn data<'a>(call: &Call) -> &'a mut Data {
    // SAFETY: the class's methods are non-static, so `$this` is one of ours.
    unsafe { Port::data(call.this()) }
}

/// `php_swoole_server_port_get_and_check_*`: an `E_ERROR`, which does not
/// return, when the port is not bound to a server. The caller owns nothing.
fn invalid(call: &Call) {
    // SAFETY: `$this` of this call; the frames above own nothing to drop.
    unsafe { (host().invalid)(call.this()) };
}

fn construct(call: &Call) {
    // SAFETY: in a handler.
    unsafe { throw_error(c"please use the Swoole\\Server->listen method") };
    call.set_bool(false);
}

fn destruct(_: &Call) {}

fn on(call: &Call) {
    let property = &mut data(call).property;
    if property.serv.is_null() {
        invalid(call);
    }
    let serv = property.serv;
    // SAFETY: the port's server.
    if unsafe { (host().started)(serv) } {
        // SAFETY: a warning; no name.
        unsafe { (host().warn)(0, ptr::null()) };
        return call.set_bool(false);
    }
    let Ok(name) = call.arity(2, 2).and_then(|()| call.string(1, b"")) else {
        return call.set_bool(false);
    };
    let cb = call.zval(2);
    // SAFETY: the argument's bytes for this call.
    let event = unsafe { swr_v1_ports_event(name.as_ptr(), name.len(), 1) };
    let Ok(index) = usize::try_from(event.index) else {
        // SAFETY: `Z_PARAM_STRING`'s bytes are NUL-terminated.
        unsafe { (host().warn)(1, name.as_ptr().cast()) };
        return call.set_bool(false);
    };
    // SAFETY: `$this`, the catalog's property name and the callback argument.
    unsafe {
        zend_update_property(
            PORT.entry(),
            call.this(),
            event.property.data.cast(),
            event.property.len,
            cb,
        );
    }
    let slot = &mut property.callbacks[index];
    if !slot.is_null() {
        // SAFETY: a callable this port created.
        unsafe { (host().callable_free)(*slot) };
    }
    // SAFETY: the callback argument.
    let callable = unsafe { (host().callable_create)(cb) };
    if callable.is_null() {
        return call.set_bool(false);
    }
    *slot = callable;
    let hook = swr_v1_ports_callback_hook(event.index);
    // SAFETY: the port's server.
    unsafe { (host().hook)(serv, hook) };
    call.set_bool(true);
}

fn get_callback(call: &Call) {
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    // SAFETY: `zend::String(name)`: an owned reference, released below.
    let name = unsafe { swr_zval_get_string(call.zval(1)) };
    // SAFETY: the reference is live until released.
    let name_bytes = unsafe { bytes(name) };
    // SAFETY: the name's bytes.
    let property = unsafe { swr_v1_ports_callback_property(name_bytes.as_ptr(), name_bytes.len()) };
    // SAFETY: releasing the reference taken above.
    unsafe { swr_zend_string_release(name) };
    if !property.data.is_null() {
        let mut rv = MaybeUninit::<zval>::uninit();
        // SAFETY: `$this`, the catalog's property name and a writable scratch zval.
        let value = unsafe {
            zend_read_property(
                PORT.entry(),
                call.this(),
                property.data.cast(),
                property.len,
                true,
                rv.as_mut_ptr(),
            )
        };
        // SAFETY: Zend returned a live zval.
        if unsafe { type_of(value) } != IS_NULL {
            // SAFETY: `RETURN_ZVAL(property, 1, 0)`.
            return unsafe { swr_zval_copy(call.return_value(), value) };
        }
    }
    call.set_null();
}

fn get_socket(call: &Call) {
    let port = data(call).port;
    if port.is_null() {
        invalid(call);
    }
    // SAFETY: a bound port and this call's return slot.
    unsafe { (host().socket)(port, call.return_value()) };
}

fn set(call: &Call) {
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    let data = data(call);
    if data.port.is_null() || data.property.serv.is_null() {
        invalid(call);
    }
    let property = (&raw mut data.property).cast::<c_void>();
    // SAFETY: an array argument.
    let table = unsafe { swoole_rs_zend::value::table(settings) };
    // SAFETY: a bound port, its property and the settings array.
    if !unsafe { (host().settings)(data.port, property, table) } {
        return call.set_bool(false);
    }
    // SAFETY: `$this`, the settings argument and the port's property.
    if unsafe { (host().finish)(call.this(), settings, property) } {
        // SAFETY: re-raise the bailout C++ trapped, from the handler, which owns nothing.
        unsafe { _zend_bailout(c"crates/phpserver/src/port.rs".as_ptr().cast_mut(), line!()) };
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_server_port___construct => construct;
    zim_swoole_server_port___destruct => destruct;
    zim_swoole_server_port_on => on;
    zim_swoole_server_port_set => set;
    zim_swoole_server_port_getCallback => get_callback;
    zim_swoole_server_port_getSocket => get_socket;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_object_mirrors_server_port_object() {
        // `struct ServerPortObject { ListenPort *port; ServerPortProperty property; zend_object std; }`
        assert_eq!(size_of::<Data>(), 16 * size_of::<*mut c_void>());
        assert_eq!(Port::OFFSET, size_of::<Data>(), "std follows the property");
    }
}
