//! `Swoole\Server\Port`: registration, objects and every method over the
//! `ListenPort` and Server these configure.
use crate::{
    callable::Callable,
    port_settings::{self, Source as _},
    settings::{Settings, Stop},
};
use std::{
    cell::{Cell, OnceCell, RefCell},
    ffi::c_void,
    ptr,
    sync::atomic::Ordering,
};
use swoole_rs_phpstubs::server_port;
use swoole_rs_ports::{PropertyDefault, class_properties};
use swoole_rs_server::{SharedServer, view};
use swoole_rs_zend::{
    Call, Custom, CustomClass, E_WARNING, Minit, Owned, Value, ZString, bailout, message,
    sys::{ZEND_ACC_PRIVATE, ZEND_ACC_PUBLIC},
};
use swoole_rs_zend::{method_table, sys as z};

/// `PHP_SWOOLE_SERVER_PORT_CALLBACK_NUM`.
const CALLBACKS: usize = swoole_rs_ports::PortCallback::ALL.len();

/// `swoole::ServerPortProperty`: the port's callables and its server. The
/// server's dispatch reads it through the port's link, and PHP code a method
/// runs can call this object's methods, so every field is a cell and the
/// property is only ever borrowed shared.
pub struct PortProperty {
    callbacks: [RefCell<Option<Callable>>; CALLBACKS],
    /// The port's share in its server, once `new_object` bound it.
    serv: OnceCell<SharedServer>,
    /// Whether the port names its server: from `new_object` until `release`.
    bound: Cell<bool>,
}

impl PortProperty {
    /// `callbacks[kind]`: the callable, a reference the call holds, or none.
    pub(crate) fn callback(&self, kind: swoole_rs_ports::PortCallback) -> Option<Callable> {
        let slot = self.callbacks.get(kind.index())?;
        slot.borrow().clone()
    }
}

/// `ServerPortObject` without its `zend_object`: the `ListenPort` and property.
pub struct PortData {
    /// Which of the server's ports, by address; null before `new_object` and
    /// after `release`.
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
                serv: OnceCell::new(),
                bound: Cell::new(false),
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
    /// The bound server: none before `new_object` binds it and after `release`.
    fn server(&self) -> Option<view::Server<'_>> {
        let serv = self
            .property
            .serv
            .get()
            .filter(|_| self.property.bound.get());
        serv.map(SharedServer::view)
    }

    /// The bound `ListenPort`, one of its server's: none before `new_object`
    /// binds it and after `release`.
    fn port(&self) -> Option<view::Port<'_>> {
        crate::server::port_of(self.property.serv.get()?.view(), self.port.get())
    }

    /// `php_swoole_server_port_deref`: drop the callables and the port's link,
    /// all of them even when a destructor leaves a bailout pending.
    fn release(&self) {
        if self.property.bound.get() {
            for slot in &self.property.callbacks {
                // Out of its slot first: a destructor this runs may reach the slot again.
                drop(slot.take());
            }
            self.property.bound.set(false);
        }
        if let Some(port) = self.port()
            && crate::lifecycle::owns_shared()
        {
            let config = port.config();
            config
                .protocol
                .update(|protocol| protocol.private_data_1 = ptr::null_mut());
            // Out of the port first: a destructor this runs may reach it again.
            drop(config.length_owner.replace(None));
        }
        self.port.set(ptr::null_mut());
    }
}

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
    let socket = swoole_rs_phpswoole::socket_create(fd).0;
    // The reference upstream's `sw_zval_dup` leaks.
    #[expect(
        clippy::mem_forget,
        reason = "the reference upstream's sw_zval_dup leaks"
    )]
    std::mem::forget(socket.copy());
    call.set_owned(socket);
}

/// A new Port object bound to `port`, one of the ports of its server `serv`.
pub(crate) fn new_object(serv: SharedServer, port: view::Port<'_>) -> Owned {
    let zport = PORT.create_object();
    let Some(data) = zport.custom::<PortData>() else {
        return zport;
    };
    data.port.set(port.raw());
    // A new object has no server yet.
    data.property.serv.get_or_init(|| serv);
    data.property.bound.set(true);
    let config = port.config();
    // The object's property lives as long as the port's link, which only reads it.
    let property = ptr::from_ref(&data.property).cast_mut().cast();
    crate::server::port_property(config.object_id, &config.ptr, property, true);
    zport
}

/// The `ListenPort` of the Port object `object` holds; an `E_ERROR`, which does
/// not return, when it is none or unbound. The caller owns nothing to drop.
pub(crate) fn bound_port(object: &Value) -> view::Port<'_> {
    let data = object.custom::<PortData>();
    data.and_then(PortData::port).unwrap_or_else(|| {
        // Only this class's `create_object` makes the Server's Port objects.
        crate::server::invalid(c"Swoole\\Server\\Port");
        // The `E_ERROR` above bailed out.
        swoole_rs_corebase::fatal("E_ERROR returned")
    })
}

/// [`PortData::release`] of a Port object the Server keeps.
pub(crate) fn release(object: &Value) {
    if let Some(data) = object.custom::<PortData>() {
        data.release();
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
        METHODS.table()
    } else {
        METHODS.without(&[c"getSocket"])
    };
    minit.register_object::<PortData>(c"Swoole\\Server\\Port", methods, 0);
    for property in class_properties() {
        let access = if property.private {
            ZEND_ACC_PRIVATE
        } else {
            ZEND_ACC_PUBLIC
        };
        let name = std::ffi::CString::new(property.name).unwrap_or_default();
        match property.value {
            PropertyDefault::Long(value) => minit.property_long(&PORT, &name, value, access),
            PropertyDefault::Bool(value) => minit.property_bool(&PORT, &name, value, access),
            PropertyDefault::Null => minit.property_null(&PORT, &name, access),
        }
    }
}

/// `$this`'s data: the class's methods are non-static, so `$this` is one of ours.
fn data(call: &Call) -> &PortData {
    call.custom().unwrap_or_else(|| {
        invalid(call);
        // The `E_ERROR` above bailed out.
        swoole_rs_corebase::fatal("E_ERROR returned")
    })
}

/// `php_swoole_server_port_get_and_check_*`: an `E_ERROR`, which does not
/// return, when the port is not bound to a server. The caller owns nothing.
fn invalid(call: &Call) {
    // `$this` of this call; the frames above own nothing to drop.
    crate::server::invalid(crate::server::this_class(call));
}

/// The port's bound server; an `E_ERROR`, which does not return, when unbound.
fn bound_server(call: &Call) -> view::Server<'_> {
    data(call).server().unwrap_or_else(|| {
        invalid(call);
        // The `E_ERROR` above bailed out.
        swoole_rs_corebase::fatal("E_ERROR returned")
    })
}

fn construct(call: &Call) {
    call.throw_error(c"please use the Swoole\\Server->listen method");
    call.set_bool(false);
}

fn destruct(_: &Call) {}

fn on(call: &Call) {
    let slots = &data(call).property.callbacks;
    let serv = bound_server(call);
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
    let Some(slot) = slots.get(index.index()) else {
        return call.set_bool(false);
    };
    drop(slot.take());
    if swoole_rs_zend::bailout::pending() {
        return;
    }
    let Some(callable) = crate::callable::create(cb) else {
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
    crate::dispatch::port_hook(serv, index);
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
    let Some(port) = data(call).port() else {
        return invalid(call);
    };
    socket(port, call);
}

/// `\Swoole\Server\Helper::checkOptions($settings)` when the library is
/// enabled, then the merge into `$this->setting`.
fn finish(call: &Call, settings: &Value) -> Result<(), bailout::Bailout> {
    if swoole_rs_phpswoole::enable_library() {
        swoole_rs_zend::host::named_call(br"\Swoole\Server\Helper::checkOptions", &[settings])?;
    }
    if let Some(props) = call.props(Some(&PORT)) {
        props.merge(c"setting", settings);
    }
    bailout::outcome(())
}

fn set(call: &Call) {
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    let port = data(call).port().unwrap_or_else(|| {
        invalid(call);
        // The `E_ERROR` above bailed out.
        swoole_rs_corebase::fatal("E_ERROR returned")
    });
    let serv = bound_server(call);
    if apply_settings(port, serv, settings).is_err() {
        return call.set_bool(false);
    }
    bailout::raise(finish(call, settings));
}

/// The `ListenPort` and TLS options of `set()`.
fn apply_settings(port: view::Port<'_>, serv: view::Server<'_>, vht: &Value) -> Result<(), Stop> {
    let source = port_settings::Zend {
        settings: Settings {
            serv,
            vht: vht.array(),
        },
        port,
    };
    port_settings::apply(&source)?;
    // A reference of this call's own, which keeps the context live throughout.
    let (ssl, kind, context) = source.config(|c| (c.ssl, c.r#type, c.ssl_context.get()));
    let Some(context) = context.filter(|_| ssl) else {
        return Ok(());
    };
    port_settings::tls(&source, &context, port_settings::is_dgram(kind))
}

swoole_rs_zend::handlers! {
    zim_swoole_server_port___construct => construct;
    zim_swoole_server_port___destruct => destruct;
    zim_swoole_server_port_on => on;
    zim_swoole_server_port_set => set;
    zim_swoole_server_port_getCallback => get_callback;
    zim_swoole_server_port_getSocket => get_socket;
}
