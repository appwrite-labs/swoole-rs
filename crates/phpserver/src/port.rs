//! `Swoole\Server\Port`: registration, objects and every method over the
//! `ListenPort` and Server these configure.
use crate::{
    callable::Callable,
    port_settings::{self, Source as _},
    settings::{Settings, Stop},
};
use std::{
    cell::{Cell, RefCell},
    ffi::{c_char, c_int, c_void},
    ptr,
    sync::atomic::Ordering,
};
use swoole_rs_phpstubs::server_port;
use swoole_rs_ports::{swr_v1_ports_callback_hook, swr_v1_ports_class_property};
use swoole_rs_server::{port_config, view};
use swoole_rs_zend::{
    Call, Custom, CustomClass, E_WARNING, Minit, Owned, Value, ZString, bailout, custom_of,
    message,
    sys::{
        ZEND_ACC_PRIVATE, ZEND_ACC_PUBLIC, swr_zval_copy, zend_class_entry,
        zend_declare_property_bool, zend_declare_property_long, zend_declare_property_null,
        zend_object, zval,
    },
};
use swoole_rs_zend::{method_table, sys as z};

/// `PHP_SWOOLE_SERVER_PORT_CALLBACK_NUM`.
const CALLBACKS: usize = 12;

/// `swoole::ServerPortProperty`: the port's callables and its server. The
/// server's dispatch reads it through the port's link, and PHP code a method
/// runs can call this object's methods, so every field is a cell and the
/// property is only ever borrowed shared.
pub struct PortProperty {
    callbacks: [RefCell<Option<Callable>>; CALLBACKS],
    serv: Cell<*mut c_void>,
}

impl PortProperty {
    /// `callbacks[kind]`, or null.
    pub(crate) fn callback(&self, kind: c_int) -> *mut c_void {
        usize::try_from(kind)
            .ok()
            .and_then(|kind| Some(self.callbacks.get(kind)?.borrow().as_ref()?.as_raw()))
            .unwrap_or(ptr::null_mut())
    }
}

/// `ServerPortObject` without its `zend_object`: the `ListenPort` and property.
pub struct PortData {
    port: Cell<*mut c_void>,
    property: PortProperty,
}

impl Custom for PortData {
    fn class() -> &'static CustomClass<Self> {
        &PORT
    }

    /// A port not yet bound to a server.
    fn new() -> Self {
        Self {
            port: Cell::new(ptr::null_mut()),
            property: PortProperty {
                callbacks: Default::default(),
                serv: Cell::new(ptr::null_mut()),
            },
        }
    }

    /// In thread mode only, as upstream (`#ifndef SW_THREAD`).
    fn serializable() -> bool {
        crate::server::THREAD.load(Ordering::Relaxed)
    }
}

impl Drop for PortData {
    fn drop(&mut self) {
        self.release();
    }
}

impl PortData {
    /// `php_swoole_server_port_deref`: drop the callables and the port's link,
    /// all of them even when a destructor leaves a bailout pending.
    fn release(&self) {
        if !self.property.serv.get().is_null() {
            for slot in &self.property.callbacks {
                // Out of its slot first: a destructor this runs may reach the slot again.
                drop(slot.take());
            }
            self.property.serv.set(ptr::null_mut());
        }
        let port = self.port.get();
        if !port.is_null() {
            if crate::lifecycle::owns_shared() {
                // SAFETY: a bound port, which outlives this call, on the thread that owns its slots.
                free_callable(take(
                    &mut unsafe { port_config::of(port) }.protocol.private_data_1,
                ));
            }
            self.port.set(ptr::null_mut());
        }
    }
}

/// The kinds `swoole_rs_ports::swr_v1_ports_class_property` returns.
const PROPERTY_END: u32 = 0;
const PROPERTY_LONG: u32 = 2;
const PROPERTY_BOOL: u32 = 3;

pub(crate) static PORT: CustomClass<PortData> = CustomClass::new();

/// `getSocket()` of a bound port: a `Socket` over a duplicate of its
/// descriptor, kept alive by an extra reference as upstream's
/// `php_swoole_convert_to_socket`; false if `dup` fails. Registered only when
/// Swoole was built with ext/sockets. `port` is a live `ListenPort`.
pub(crate) fn socket(port: swoole_rs_server::view::Port<'_>, call: &Call) {
    let fd = port.fd();
    let fd = swoole_rs_corebase::sys::dup(fd);
    if fd < 0 {
        return call.set_bool(false);
    }
    call.set_owned(swoole_rs_phpswoole::socket_create(fd).0);
    // SAFETY: an emalloc'd slot for the reference upstream leaks.
    let keep = unsafe { z::swr_zend_emalloc(size_of::<zval>()) }.cast::<zval>();
    // SAFETY: a writable slot and the Socket just made.
    unsafe { swr_zval_copy(keep, call.return_value()) };
}

/// A new Port object bound to `port` and its server `serv`.
/// # Safety
/// `serv` is a live server and `port` one of its ports.
pub(crate) unsafe fn new_object(serv: *mut c_void, port: *mut c_void) -> (Owned, *mut zend_object) {
    let zport = PORT.create_object();
    let (Some(object), Some(data)) = (zport.object(), zport.custom::<PortData>()) else {
        return (zport, ptr::null_mut());
    };
    data.port.set(port);
    data.property.serv.set(serv);
    // SAFETY: per the caller.
    let config = unsafe { port_config::of(port) };
    // The object's property lives as long as the port's link, which only reads it.
    let property = ptr::from_ref(&data.property).cast_mut().cast();
    crate::server::port_property(config.object_id, &mut config.ptr, property, true);
    (zport, object)
}

/// The object's `ListenPort`; an `E_ERROR`, which does not return, when unbound.
/// # Safety
/// `object` is a `Swoole\Server\Port` this class created, bound for `'a`; the caller
/// owns nothing to drop.
pub(crate) unsafe fn bound_port<'a>(object: *mut zend_object) -> swoole_rs_server::view::Port<'a> {
    // SAFETY: a live object, per the contract.
    let data = unsafe { custom_of::<PortData>(object) };
    let port = data.map_or(ptr::null_mut(), |data| data.port.get());
    // SAFETY: null, or the object's live port.
    let port = unsafe { swoole_rs_server::view::Port::from_raw(port) };
    port.unwrap_or_else(|| {
        crate::server::invalid(object);
        // The `E_ERROR` above bailed out.
        std::process::abort()
    })
}

/// [`PortData::release`] of a Port object the Server keeps.
/// # Safety
/// `object` is a live object.
pub(crate) unsafe fn deref(object: *mut zend_object) {
    // SAFETY: per the caller.
    if let Some(data) = unsafe { custom_of::<PortData>(object) } {
        data.release();
    }
}

/// A slot's callable, which leaves it null.
fn take(slot: &mut *mut c_void) -> *mut c_void {
    std::mem::replace(slot, ptr::null_mut())
}

/// Free a callable already out of its slot: a destructor this runs may reach
/// the slot again, or leave a bailout pending.
fn free_callable(callable: *mut c_void) {
    if !callable.is_null() {
        // SAFETY: a callable created for this port, owned by no slot now.
        unsafe { swoole_rs_phpruntime::callable_free(callable) };
    }
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_server_port___construct, &server_port::arginfo_class_Swoole_Server_Port___construct, z::ZEND_ACC_PRIVATE;
        c"__destruct" => zim_swoole_server_port___destruct, &server_port::arginfo_class_Swoole_Server_Port___destruct, z::ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_server_port_set, &server_port::arginfo_class_Swoole_Server_Port_set, z::ZEND_ACC_PUBLIC;
        c"on" => zim_swoole_server_port_on, &server_port::arginfo_class_Swoole_Server_Port_on, z::ZEND_ACC_PUBLIC;
        c"getCallback" => zim_swoole_server_port_getCallback, &server_port::arginfo_class_Swoole_Server_Port_getCallback, z::ZEND_ACC_PUBLIC;
        // SWOOLE_SOCKETS_SUPPORT builds only; MINIT drops it from the others.
        c"getSocket" => zim_swoole_server_port_getSocket, &server_port::arginfo_class_Swoole_Server_Port_getSocket, z::ZEND_ACC_PUBLIC;
    ];
}

/// MINIT: register the class as `php_swoole_server_port_minit` did; `sockets`
/// is `SWOOLE_SOCKETS_SUPPORT`.
pub(crate) fn minit(minit: &Minit, sockets: bool) {
    let methods = if sockets {
        &METHODS.0
    } else {
        METHODS.without(&[c"getSocket"])
    };
    minit.register_object::<PortData>(c"Swoole\\Server\\Port", methods, 0);
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
}

/// `$this`'s data: the class's methods are non-static, so `$this` is one of ours.
fn data(call: &Call) -> &PortData {
    call.custom().unwrap_or_else(|| {
        invalid(call);
        // The `E_ERROR` above bailed out.
        std::process::abort()
    })
}

/// `php_swoole_server_port_get_and_check_*`: an `E_ERROR`, which does not
/// return, when the port is not bound to a server. The caller owns nothing.
fn invalid(call: &Call) {
    // `$this` of this call; the frames above own nothing to drop.
    crate::server::invalid(call.this());
}

fn construct(call: &Call) {
    call.throw_error(c"please use the Swoole\\Server->listen method");
    call.set_bool(false);
}

fn destruct(_: &Call) {}

fn on(call: &Call) {
    let slots = &data(call).property.callbacks;
    let serv = data(call).property.serv.get();
    if serv.is_null() {
        invalid(call);
    }
    // SAFETY: the port's bound server, live while the port is.
    let serv = unsafe { view::Server::from_raw(serv) };
    if crate::io::closed(serv) {
        crate::server::fatal(
            E_WARNING,
            c"can't register event callback function after server started",
        );
        return call.set_bool(false);
    }
    let Ok(name) = call.arity(2, 2).and_then(|()| call.string(1, b"")) else {
        return call.set_bool(false);
    };
    let Some((index, property)) = swoole_rs_ports::event(name, true) else {
        let name =
            String::from_utf8_lossy(name.split(|&byte| byte == 0).next().unwrap_or_default());
        crate::lifecycle::runtime_warning(message!("unknown event types[{name}]").as_cstr());
        return call.set_bool(false);
    };
    let Some(cb) = call.value(2) else {
        return call.set_bool(false);
    };
    // The Port class's own private property, which runs no PHP code.
    bailout::defer(call.update_property(Some(&PORT), property, cb));
    if bailout::pending() {
        return;
    }
    // The property write, the release and the create can each run PHP that
    // calls on() again, so no borrow of the slot lives across them.
    let Some(slot) = slots.get(index) else {
        return call.set_bool(false);
    };
    drop(slot.take());
    if swoole_rs_zend::bailout::pending() {
        return;
    }
    let Some(callable) = Callable::create(cb) else {
        return call.set_bool(false);
    };
    // A nested on() stored its callable, and the property, after this one's
    // write: keep that pair and drop ours.
    let kept = if slot.borrow().is_none() {
        slot.replace(Some(callable))
    } else {
        Some(callable)
    };
    drop(kept);
    let event = i32::try_from(index).unwrap_or(-1);
    crate::dispatch::port_hook(serv, swr_v1_ports_callback_hook(event));
    call.set_bool(true);
}

fn get_callback(call: &Call) {
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    let name = call.value(1).map(Value::string);
    let name = name.as_ref().map_or(&[][..], ZString::bytes);
    match swoole_rs_ports::callback_property(name) {
        // `RETURN_ZVAL(property, 1, 0)`, null when unset.
        Some(property) => call.set_property(Some(&PORT), property),
        None => call.set_null(),
    }
}

fn get_socket(call: &Call) {
    // SAFETY: null, or the object's live port.
    let Some(port) = (unsafe { swoole_rs_server::view::Port::from_raw(data(call).port.get()) })
    else {
        return invalid(call);
    };
    socket(port, call);
}

/// `zsetting = sw_zend_read_and_convert_property_array(scope, object, "setting")`,
/// then `SEPARATE_ARRAY(zsetting); php_array_merge(zsetting, settings)`: the
/// merged property.
pub(crate) fn merge_setting(
    scope: *mut zend_class_entry,
    object: *mut zend_object,
    settings: *mut zval,
) -> Result<*mut zval, bailout::Bailout> {
    // SAFETY: the object and its class, which declares `setting`.
    let setting = unsafe { swoole_rs_zend::host::array_property_of(scope, object, c"setting") }?;
    // SAFETY: the property, now an array, and the settings array.
    bailout::trapped(unsafe { z::swr_zend_try_array_merge(setting, settings) })?;
    Ok(setting)
}

/// `\Swoole\Server\Helper::checkOptions($settings)` when the library is
/// enabled, then the merge into `$this->setting`.
fn finish(call: &Call, settings: &Value) -> Result<(), bailout::Bailout> {
    if swoole_rs_phpswoole::enable_library() {
        // `zval params[1] = {*zset}`: a copy without a reference.
        // SAFETY: the live settings argument.
        let mut params = [unsafe { settings.as_ptr().read() }];
        // SAFETY: as above.
        unsafe {
            swoole_rs_zend::host::named_call(br"\Swoole\Server\Helper::checkOptions", &mut params)
        }?;
    }
    merge_setting(PORT.entry(), call.this(), settings.as_ptr()).map(drop)
}

fn set(call: &Call) {
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    let data = data(call);
    let (port, serv) = (data.port.get(), data.property.serv.get());
    if port.is_null() || serv.is_null() {
        invalid(call);
    }
    // SAFETY: the port's bound server, live while the port is.
    let serv = unsafe { view::Server::from_raw(serv) };
    if apply_settings(port, serv, settings).is_err() {
        return call.set_bool(false);
    }
    bailout::raise(finish(call, settings));
}

/// The `ListenPort` and TLS options of `set()`.
fn apply_settings(port: *mut c_void, serv: view::Server<'_>, vht: &Value) -> Result<(), Stop> {
    let source = port_settings::Zend {
        settings: Settings {
            serv,
            vht: vht.array(),
        },
        port,
    };
    port_settings::apply(&source)?;
    let (ssl, kind, context) = source.config(|c| {
        let context = c.ssl_context.as_ref();
        let context = context.map_or(core::ptr::null(), swoole_rs_tls::TlsContext::as_ptr);
        (c.ssl, c.r#type, context)
    });
    if ssl == 0 {
        return Ok(());
    }
    port_settings::tls(&source, context, port_settings::is_dgram(kind))
}

swoole_rs_zend::handlers! {
    zim_swoole_server_port___construct => construct;
    zim_swoole_server_port___destruct => destruct;
    zim_swoole_server_port_on => on;
    zim_swoole_server_port_set => set;
    zim_swoole_server_port_getCallback => get_callback;
    zim_swoole_server_port_getSocket => get_socket;
}
