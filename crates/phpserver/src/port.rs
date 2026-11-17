//! `Swoole\Server\Port`: registration, objects and every method over the
//! `ListenPort` and Server these configure.
use crate::{
    callable::Callable,
    port_settings::{self, Source as _},
    settings::{Settings, Stop},
};
use std::{
    cell::{Cell, OnceCell},
    ffi::c_void,
    ptr,
    rc::{Rc, Weak},
    sync::atomic::Ordering,
};
use swoole_rs_corebase::CellExt;
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

/// The port's callables, shared with dispatch through the port's slot (or
/// [`PROPERTIES`] in thread mode). Server ownership stays in [`PortData`] so
/// the dispatch reference cannot form a cycle. A callback slot is taken out
/// and put back with nothing in between that runs PHP code.
pub struct PortProperty {
    callbacks: [Cell<Option<Callable>>; CALLBACKS],
}

impl PortProperty {
    /// Select a callable while its slot is temporarily taken, without running PHP.
    #[inline]
    fn select<R>(
        &self,
        kind: swoole_rs_ports::PortCallback,
        select: impl FnOnce(&Option<Callable>) -> R,
    ) -> R {
        let Some(slot) = self.callbacks.get(kind.index()) else {
            return select(&None);
        };
        let callable = slot.take();
        let selected = select(&callable);
        slot.set(callable);
        selected
    }
}

thread_local! {
    /// `php_swoole_server_{get,set}_port_property`: this thread's
    /// `ServerPortProperty` of each port, by the port's `object_id`. Upstream keeps
    /// one per thread in thread mode, where each worker thread has Port objects of
    /// its own for the shared ports, and in the port's `ptr` otherwise, where one
    /// thread runs PHP. The Port object owns its property; this only finds it.
    /// Taken out and put back around each access, which runs no PHP code.
    static PROPERTIES: Cell<Vec<Weak<PortProperty>>> = const { Cell::new(Vec::new()) };
}

/// `f` over this thread's [`PROPERTIES`], taken out and put back.
fn with_properties<R>(f: impl FnOnce(&mut Vec<Weak<PortProperty>>) -> R) -> R {
    PROPERTIES.with(|cell| cell.with_taken(f))
}

/// `php_swoole_server_get_port_property(port)->callbacks[kind]`: the callable,
/// a reference the call holds; none without one, or without a live Port object
/// of this thread's for the port.
#[inline]
pub(crate) fn callback(
    port: view::Port<'_>,
    kind: swoole_rs_ports::PortCallback,
) -> Option<Callable> {
    property(port, |property| property.select(kind, Clone::clone)).flatten()
}

/// Whether the port has a callable, without taking a reference to the callable.
#[inline]
pub(crate) fn has_callback(port: view::Port<'_>, kind: swoole_rs_ports::PortCallback) -> bool {
    property(port, |property| property.select(kind, Option::is_some)).unwrap_or(false)
}

/// Borrow the callback storage just long enough to select a slot; no PHP runs here.
#[inline(always)]
fn property<R>(port: view::Port<'_>, select: impl FnOnce(&PortProperty) -> R) -> Option<R> {
    let config = port.config();
    if crate::server::THREAD.load(Ordering::Relaxed) {
        let property = thread_property(config.object_id)?;
        Some(select(&property))
    } else {
        Some(select(
            config.property.get()?.downcast_ref::<PortProperty>()?,
        ))
    }
}

/// Keep the TLS table and temporary owner out of the shared-port lookup.
#[inline(never)]
fn thread_property(object_id: u16) -> Option<Rc<PortProperty>> {
    with_properties(|properties| {
        properties
            .get(usize::from(object_id))
            .and_then(Weak::upgrade)
    })
}

/// `ServerPortObject` without its `zend_object`: the `ListenPort` and property.
pub struct PortData {
    /// Which of the server's ports, by address; null before `new_object` and
    /// after `release`.
    port: Cell<*mut c_void>,
    property: Rc<PortProperty>,
    /// The port's share in its server, once `new_object` bound it.
    serv: OnceCell<SharedServer>,
    /// Whether the port names its server: from `new_object` until `release`.
    bound: Cell<bool>,
}

impl Custom for PortData {
    fn class() -> &'static CustomClass<Self> {
        &PORT
    }

    /// A port not yet bound to a server.
    fn new() -> Self {
        Self {
            port: Cell::new(ptr::null_mut()),
            property: Rc::new(PortProperty {
                callbacks: Default::default(),
            }),
            serv: OnceCell::new(),
            bound: Cell::new(false),
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
        let serv = self.serv.get().filter(|_| self.bound.get());
        serv.map(SharedServer::view)
    }

    /// The bound `ListenPort`, one of its server's: none before `new_object`
    /// binds it and after `release`.
    fn port(&self) -> Option<view::Port<'_>> {
        crate::server::port_of(self.serv.get()?.view(), self.port.get())
    }

    /// `php_swoole_server_port_deref`: drop the callables and the port's link,
    /// all of them even when a destructor leaves a bailout pending.
    fn release(&self) {
        if self.bound.get() {
            for slot in &self.property.callbacks {
                // Out of its slot first: a destructor this runs may reach the slot again.
                drop(slot.take());
            }
            self.bound.set(false);
        }
        if let Some(port) = self.port() {
            // This thread's owner, out of its registry first: a destructor this runs
            // may reach the port again.
            drop(port.config().replace_length_owner(None));
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
    let Ok(fd) = swoole_rs_corebase::sys::dup(fd) else {
        return call.set_bool(false);
    };
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
    data.serv.get_or_init(|| serv);
    data.bound.set(true);
    // `php_swoole_server_set_port_property`.
    let config = port.config();
    if crate::server::THREAD.load(Ordering::Relaxed) {
        let property = Rc::downgrade(&data.property);
        let index = usize::from(config.object_id);
        with_properties(|properties| {
            if properties.len() <= index {
                properties.resize(index + 1, Weak::new());
            }
            if let Some(entry) = properties.get_mut(index) {
                *entry = property;
            }
        });
    } else {
        // Each non-threaded ListenPort gets its object once: during bind, or listen.
        config.property.get_or_init(|| data.property.clone());
    }
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
    // write: keep that pair and drop ours once it is back.
    match slot.take() {
        None => slot.set(Some(callable)),
        nested => {
            slot.set(nested);
            drop(callable);
        }
    }
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
