//! `Swoole\Server\Port`: registration, objects and every method over the
//! `ListenPort` and Server these configure.
use crate::{
    callable::Callable,
    port_settings,
    settings::{Settings, Stop},
};
use std::{
    ffi::{c_char, c_int, c_void},
    mem::MaybeUninit,
    ptr,
};
use swoole_rs_phpstubs::server_port;
use swoole_rs_ports::{
    swr_v1_ports_callback_hook, swr_v1_ports_callback_property, swr_v1_ports_class_property,
    swr_v1_ports_event,
};
use swoole_rs_server::{port_config, view};
use swoole_rs_zend::{
    Call, Class, E_WARNING, Minit, Object, Owned, Value, ZString, bailout, message,
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PRIVATE, ZEND_ACC_PUBLIC, swr_zval_copy,
        zend_class_entry, zend_declare_property_bool, zend_declare_property_long,
        zend_declare_property_null, zend_object, zend_object_handlers, zend_read_property,
        zend_update_property, zval,
    },
    unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

/// `PHP_SWOOLE_SERVER_PORT_CALLBACK_NUM`.
const CALLBACKS: usize = 12;

/// `swoole::ServerPortProperty`: pointers only, so Rust lays it out as C++ does
/// (a callback slot is a `zend::Function *`, null when empty).
#[repr(C)]
pub struct PortProperty {
    callbacks: [Option<Callable>; CALLBACKS],
    serv: *mut c_void,
    port: *mut c_void,
    zsetting: *mut zval,
}

impl PortProperty {
    /// `callbacks[kind]`, or null.
    pub(crate) fn callback(&self, kind: c_int) -> *mut c_void {
        usize::try_from(kind)
            .ok()
            .and_then(|kind| self.callbacks.get(kind)?.as_ref())
            .map_or(ptr::null_mut(), Callable::as_raw)
    }
}

/// `ServerPortObject` without its `zend_object`: the `ListenPort` and property.
#[repr(C)]
pub struct PortData {
    port: *mut c_void,
    property: PortProperty,
}

type Port = Object<PortData>;

/// The kinds `swoole_rs_ports::swr_v1_ports_class_property` returns.
const PROPERTY_END: u32 = 0;
const PROPERTY_LONG: u32 = 2;
const PROPERTY_BOOL: u32 = 3;

pub(crate) static PORT: Class = Class::new();

/// `getSocket()` of a bound port: a `Socket` over a duplicate of its
/// descriptor, kept alive by an extra reference as upstream's
/// `php_swoole_convert_to_socket`; false if `dup` fails. Registered only when
/// Swoole was built with ext/sockets. `port` is a live `ListenPort`.
pub(crate) fn socket(port: *mut c_void, call: &Call) {
    // SAFETY: a live port, as above.
    let fd = unsafe { swoole_rs_server::view::port_fd(port) };
    // SAFETY: a descriptor the port owns.
    let fd = unsafe { libc::dup(fd) };
    if fd < 0 {
        return call.set_bool(false);
    }
    // SAFETY: this call's return slot, which takes `fd` whatever the result.
    unsafe { swoole_rs_phpswoole::socket_create(call.return_value(), fd) };
    // SAFETY: an emalloc'd slot for the reference upstream leaks.
    let keep = unsafe { z::swr_zend_emalloc(size_of::<zval>()) }.cast::<zval>();
    // SAFETY: a writable slot and the Socket just made.
    unsafe { swr_zval_copy(keep, call.return_value()) };
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass of it;
    // null pointers are a port not yet bound to a server.
    unsafe { Port::create(&PORT, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    unsafe { deref(std) };
    // SAFETY: as above.
    unsafe { Port::dtor(std) };
    swoole_rs_zend::bailout::finish();
}

/// A new Port object bound to `port` and its server `serv`.
/// # Safety
/// `serv` is a live server and `port` one of its ports.
pub(crate) unsafe fn new_object(serv: *mut c_void, port: *mut c_void) -> (Owned, *mut zend_object) {
    // SAFETY: the registered class.
    let zport = unsafe { Owned::new_object(PORT.entry()) };
    let object = zport.object().unwrap_or(ptr::null_mut());
    // SAFETY: the object just created, one of ours.
    let data = unsafe { Port::data(object) };
    data.port = port;
    data.property.serv = serv;
    data.property.port = port;
    // SAFETY: per the caller.
    let config = unsafe { port_config::of(port) };
    // The object's property lives as long as the port's link.
    crate::server::port_property(
        config.object_id,
        &mut config.ptr,
        (&raw mut data.property).cast(),
        true,
    );
    (zport, object)
}

/// The object's `ListenPort`; an `E_ERROR`, which does not return, when unbound.
/// # Safety
/// `object` is a `Swoole\Server\Port` this class created; the caller owns nothing to drop.
pub(crate) unsafe fn bound_port(object: *mut zend_object) -> *mut c_void {
    // SAFETY: one of ours, per the contract.
    let port = unsafe { Port::data(object) }.port;
    if port.is_null() {
        crate::server::invalid(object);
    }
    port
}

/// `php_swoole_server_port_deref`: drop the callables and the port's link,
/// all of them even when a destructor leaves a bailout pending.
/// # Safety
/// `object` is a `Swoole\Server\Port` this class created.
pub(crate) unsafe fn deref(object: *mut zend_object) {
    // SAFETY: one of ours, per the contract.
    let data = unsafe { Port::data(object) };
    if !data.property.serv.is_null() {
        for slot in &mut data.property.callbacks {
            // Out of its slot first: a destructor this runs may reach the slot again.
            drop(slot.take());
        }
        data.property.serv = ptr::null_mut();
    }
    if !data.port.is_null() {
        if crate::lifecycle::owns_shared() {
            // SAFETY: a bound port, which outlives this call, on the thread that owns its slots.
            free_callable(take(
                &mut unsafe { port_config::of(data.port) }
                    .protocol
                    .private_data_1,
            ));
        }
        data.port = ptr::null_mut();
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
pub(crate) fn minit(minit: &Minit, not_serializable: bool, sockets: bool) {
    let methods = if sockets {
        &METHODS.0
    } else {
        METHODS.without(&[c"getSocket"])
    };
    minit.register(
        &PORT,
        c"Swoole\\Server\\Port",
        methods,
        |entry, handlers| {
            configure(entry, handlers, not_serializable);
        },
    );
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

fn data<'a>(call: &Call) -> &'a mut PortData {
    // SAFETY: the class's methods are non-static, so `$this` is one of ours.
    unsafe { Port::data(call.this()) }
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
    let property = &mut data(call).property;
    if property.serv.is_null() {
        invalid(call);
    }
    // SAFETY: the port's bound server, live while the port is.
    let serv = unsafe { view::Server::from_raw(property.serv) };
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
    let cb = call.zval(2);
    // SAFETY: the argument's bytes for this call.
    let event = unsafe { swr_v1_ports_event(name.as_ptr(), name.len(), 1) };
    let Ok(index) = usize::try_from(event.index) else {
        let name =
            String::from_utf8_lossy(name.split(|&byte| byte == 0).next().unwrap_or_default());
        crate::lifecycle::runtime_warning(message!("unknown event types[{name}]").as_cstr());
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
    // The property write, the release and the create can each run PHP that
    // calls on() again, so no borrow of the slot lives across them.
    let Some(old) = data(call)
        .property
        .callbacks
        .get_mut(index)
        .map(Option::take)
    else {
        return call.set_bool(false);
    };
    drop(old);
    if swoole_rs_zend::bailout::pending() {
        return;
    }
    let Some(callable) = call.value(2).and_then(Callable::create) else {
        return call.set_bool(false);
    };
    let kept = match data(call).property.callbacks.get_mut(index) {
        // A nested on() stored its callable, and the property, after this one's
        // write: keep that pair and drop ours.
        Some(slot) if slot.is_none() => slot.replace(callable),
        _ => Some(callable),
    };
    drop(kept);
    crate::dispatch::port_hook(serv, swr_v1_ports_callback_hook(event.index));
    call.set_bool(true);
}

fn get_callback(call: &Call) {
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    let name = call.value(1).map(Value::string);
    let name = name.as_ref().map_or(&[][..], ZString::bytes);
    // SAFETY: the name's bytes.
    let property = unsafe { swr_v1_ports_callback_property(name.as_ptr(), name.len()) };
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
        // SAFETY: Zend returned a live zval, which the object or `rv` holds.
        if let Some(value) = unsafe { Value::from_ptr(value) }.filter(|value| !value.is_null()) {
            // `RETURN_ZVAL(property, 1, 0)`.
            return call.set_owned(value.copy());
        }
    }
    call.set_null();
}

fn get_socket(call: &Call) {
    let port = data(call).port;
    if port.is_null() {
        invalid(call);
    }
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
    let mut zobject = crate::dispatch::undef();
    // SAFETY: a borrowed view of the live object (`ZVAL_OBJ`, no reference).
    unsafe { swoole_rs_zend::set_object(&raw mut zobject, object) };
    // SAFETY: the object and its class, which declares `setting`.
    let setting =
        unsafe { swoole_rs_zend::host::array_property_of(scope, &raw mut zobject, c"setting") }?;
    // SAFETY: the property, now an array, and the settings array.
    bailout::trapped(unsafe { z::swr_zend_try_array_merge(setting, settings) })?;
    Ok(setting)
}

/// `\Swoole\Server\Helper::checkOptions($settings)` when the library is
/// enabled, then the merge into `$this->setting`, which the property keeps.
fn finish(
    call: &Call,
    settings: &Value,
    property: &mut PortProperty,
) -> Result<(), bailout::Bailout> {
    if swoole_rs_phpswoole::enable_library() {
        // `zval params[1] = {*zset}`: a copy without a reference.
        // SAFETY: the live settings argument.
        let mut params = [unsafe { settings.as_ptr().read() }];
        // SAFETY: as above.
        unsafe {
            swoole_rs_zend::host::named_call(br"\Swoole\Server\Helper::checkOptions", &mut params)
        }?;
    }
    property.zsetting = merge_setting(PORT.entry(), call.this(), settings.as_ptr())?;
    Ok(())
}

fn set(call: &Call) {
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    let data = data(call);
    if data.port.is_null() || data.property.serv.is_null() {
        invalid(call);
    }
    // SAFETY: the port's bound server, live while the port is.
    let serv = unsafe { view::Server::from_raw(data.property.serv) };
    if apply_settings(data.port, serv, settings).is_err() {
        return call.set_bool(false);
    }
    bailout::raise(finish(call, settings, &mut data.property));
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
    // SAFETY: a bound port; set() is its only user while it runs.
    let config = unsafe { port_config::of(port) };
    port_settings::apply(&source, config)?;
    if config.ssl == 0 {
        return Ok(());
    }
    let dgram = port_settings::is_dgram(config.r#type);
    port_settings::tls(&source, config.ssl_context, dgram)
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
        assert_eq!(size_of::<PortData>(), 16 * size_of::<*mut c_void>());
        assert_eq!(
            Port::OFFSET,
            size_of::<PortData>(),
            "std follows the property"
        );
    }
}
