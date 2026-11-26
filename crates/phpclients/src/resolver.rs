//! `Swoole\NameResolver\Context` and the `swoole_name_resolver_*` functions:
//! registration, objects, the functions' bodies and the PHP resolver's lookup
//! (a `Swoole\NameResolver` object's `lookup()`, then its cluster's `pop()`).
//! The context and the resolver list are crates/corebase's (`NameResolverContext`,
//! `swr_v1_core_name_resolver_*`), shared with the coroutine socket's lookup; a PHP
//! resolver's entry names its object, whose reference crates/phpcoroutines keeps.
use crate::socket::warn;
pub use native::swoole_name_resolver_php_remove;
use std::{
    cell::Cell,
    ffi::{CString, c_void},
};
use swoole_rs_corebase::{CCast, NameResolverContext, ResolverOptions};
use swoole_rs_phpstubs::name_resolver;
use swoole_rs_zend::{
    Call, Custom, CustomClass, Minit, Owned, Value, ZString, bailout, method_table,
    sys::{IS_ARRAY, IS_FALSE, IS_NULL, IS_OBJECT, IS_STRING, ZEND_ACC_PUBLIC},
};

/// `AF_INET`, `__construct`'s default family.
const AF_INET: i64 = libc::AF_INET as i64;
/// `SW_ZSTR_KNOWN(SW_ZEND_STR_CLASS_NAME_RESOLVER)`.
const RESOLVER_CLASS: &[u8] = b"Swoole\\NameResolver";

static CONTEXT: CustomClass<Held> = CustomClass::new();

/// A `NameResolver::Context` (crates/corebase's `NameResolverContext`) and the resolver
/// list, as the rest of this file uses them.
mod native {
    use std::ffi::c_void;
    use swoole_rs_corebase::{NameResolverContext, ResolverOptions, TYPE_PHP};
    use swoole_rs_phpcoroutines::name_resolvers;
    use swoole_rs_zend::{Owned, Value};

    /// Reset the lookup and release any retained cluster.
    pub(crate) fn swoole_name_resolver_context_reset(
        context: &mut NameResolverContext,
        with_port: bool,
    ) {
        *context = NameResolverContext {
            options: ResolverOptions::Owned(context.options.get().0, with_port),
            timeout: context.timeout,
            ..NameResolverContext::default()
        };
    }

    /// `swoole_name_resolver_add({lookup, resolver, TYPE_PHP})` for an object: the
    /// list's entry names the object, whose reference this thread keeps
    /// (crates/phpcoroutines) until a remove unlinks the entry.
    pub(super) fn swoole_name_resolver_php_add(resolver: Owned) {
        let Some(object) = resolver.object() else {
            return;
        };
        name_resolvers(|kept| kept.push(resolver));
        swoole_rs_corebase::swr_v1_core_name_resolver_add(
            super::lookup_php,
            object.cast(),
            TYPE_PHP,
            true,
        );
    }

    /// Where a kept reference to the object `data` names is, if this thread keeps one.
    pub(super) fn position(kept: &[Owned], data: *mut c_void) -> Option<usize> {
        kept.iter().position(|resolver| {
            resolver
                .object()
                .is_some_and(|object| object.cast() == data)
        })
    }

    /// Remove this thread's first PHP resolver for `resolver`'s object (any, when
    /// `None`) and return the reference kept for it, or none if there was none (or
    /// `resolver` is no object).
    pub fn swoole_name_resolver_php_remove(resolver: Option<&Value>) -> Option<Owned> {
        let object = match resolver.map(Value::object) {
            None => None,
            Some(Some(object)) => Some(object),
            Some(None) => return None,
        };
        let same = |data: *mut c_void, kind| {
            kind == TYPE_PHP
                && object.is_none_or(|object| data == object.cast())
                && name_resolvers(|kept| position(kept, data).is_some())
        };
        let data = swoole_rs_corebase::swr_v1_core_name_resolver_remove(same);
        name_resolvers(|kept| position(kept, data).map(|at| kept.remove(at)))
    }
}

pub(crate) use native::swoole_name_resolver_context_reset;

/// Taken while a PHP resolver callback is using this context.
struct Held(Cell<Option<NameResolverContext>>);

impl Custom for Held {
    fn class() -> &'static CustomClass<Self> {
        &CONTEXT
    }
    fn new() -> Self {
        Self(Cell::new(Some(NameResolverContext::default())))
    }
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_name_resolver_context___construct, &name_resolver::arginfo_class_Swoole_NameResolver_Context___construct, ZEND_ACC_PUBLIC;
    ];
}

/// MINIT: register `Swoole\NameResolver\Context`.
/// # Errors
/// A registration bailed out.
pub fn php_swoole_name_resolver_minit(minit: &Minit) -> Result<(), bailout::Bailout> {
    minit.register_object::<Held>(c"Swoole\\NameResolver\\Context", METHODS.table(), 0);
    Ok(())
}

/// `__construct(int $family = AF_INET, bool $withPort = false)`.
fn construct(call: &Call) {
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, AF_INET)?, call.bool(2, false)?)));
    let Ok((family, with_port)) = parsed else {
        call.set_bool(false);
        return;
    };
    if family != AF_INET && family != i64::from(libc::AF_INET6) {
        return call.argument_value_error(1, c"must be AF_INET or AF_INET6");
    }
    let Some(held) = call.custom::<Held>() else {
        return crate::native::unconstructed();
    };
    let Some(previous) = held.0.take() else {
        return call.throw_error(c"NameResolver Context is already in use");
    };
    drop(previous);
    held.0.set(Some(NameResolverContext {
        options: ResolverOptions::Owned(family.c_cast(), with_port),
        ..NameResolverContext::default()
    }));
}

/// `swoole_name_resolver_lookup(string $name, Swoole\NameResolver\Context $ctx): string`.
fn lookup(call: &Call) {
    let Ok(name) = call.arity(2, 2).and_then(|()| call.string(1, b"")) else {
        return call.set_bool(false);
    };
    let Some(argument) = call.value(2).map(Value::dereferenced) else {
        return;
    };
    let Some(held) = argument.custom::<Held>() else {
        return call.wrong_class(2, c"Swoole\\NameResolver\\Context", &argument);
    };
    if name.is_empty() || name.contains(&0) {
        return call.argument_value_error(1, c"must be nonempty and contain no null bytes");
    }
    let Some(mut context) = held.0.take() else {
        return call.throw_error(c"NameResolver Context is already in use");
    };
    let mut result = Vec::new();
    let looked_up = swoole_rs_zend::contain_with(|| {
        swoole_rs_hooks::resolve::name_resolver_lookup(name, &mut context, &mut result);
    });
    held.0.set(Some(context));
    match looked_up {
        Ok(()) if swoole_rs_zend::host::exception().is_null() => {
            call.set_zstring(ZString::new(&result));
        }
        Ok(()) => {}
        Err(bailout) => bailout.defer(),
    }
}

/// `swoole_name_resolver_add(Swoole\NameResolver $ns): bool`.
fn add(call: &Call) {
    match call.arity(1, 1).and_then(|()| call.object(1)) {
        Ok(_) => call.set_bool(
            call.value(1)
                .map(Value::dereferenced)
                .as_deref()
                .is_some_and(php_swoole_name_resolver_add),
        ),
        Err(_) => call.set_bool(false),
    }
}

/// `php_swoole_name_resolver_add`: register `resolver` (an object) if it is a
/// `Swoole\NameResolver`, holding a reference to it.
pub fn php_swoole_name_resolver_add(resolver: &Value) -> bool {
    // Autoloads as upstream's known string does, inside `zend_try`.
    let instance = match resolver.instance_of_named(&ZString::new(RESOLVER_CLASS)) {
        Ok(instance) => instance,
        Err(bailout) => {
            bailout.defer();
            return false;
        }
    };
    let class = String::from_utf8_lossy(RESOLVER_CLASS);
    let Some(instance) = instance else {
        warn(true, &format!("Class \"{class}\" not found"));
        return false;
    };
    if resolver.object().is_none() {
        return false;
    }
    if !instance {
        warn(
            true,
            &format!("the given object is not an instance of {class}"),
        );
        return false;
    }
    // `sw_zval_dup`: the list keeps a reference of its own until remove or RSHUTDOWN.
    native::swoole_name_resolver_php_add(resolver.copy());
    true
}

/// `swoole_name_resolver_remove(Swoole\NameResolver $ns): bool`.
fn remove(call: &Call) {
    if call.arity(1, 1).and_then(|()| call.object(1)).is_err() {
        call.set_bool(false);
        return;
    }
    // Dropping the reference `add` took runs no destructor: the argument holds another.
    let kept =
        native::swoole_name_resolver_php_remove(call.value(1).map(Value::dereferenced).as_deref());
    call.set_bool(kept.is_some());
}

/// `php_swoole_name_resolver_lookup`: ask the `Swoole\NameResolver` object `data` names
/// (registered only by [`php_swoole_name_resolver_add`], and skipped unless this thread
/// keeps it) for `name`, keeping a returned cluster in the context and popping a node
/// from it. Writes the result to `out` and returns whether it is non-empty, then raises
/// a bailout the PHP code caused.
fn lookup_php(
    data: *mut c_void,
    name: &[u8],
    context: &mut NameResolverContext,
    out: &mut Vec<u8>,
) -> bool {
    // A reference of our own: the resolver's code may remove it.
    let resolver = swoole_rs_phpcoroutines::name_resolvers(|kept| {
        Some(kept.get(native::position(kept, data)?)?.copy())
    });
    let result =
        (resolver.as_deref()).map_or(Ok(None), |resolver| resolve(resolver, name, context));
    // Its release may run a destructor, whose bailout `finish` raises.
    drop(resolver);
    let answered = match result {
        Ok(Some(text)) => {
            *out = text;
            !out.is_empty()
        }
        Ok(None) => false,
        Err(bailout) => {
            bailout.defer();
            false
        }
    };
    answered && !bailout::pending()
}

/// Decode PHP resolver results before allowing a lookup to fall back to DNS.
fn resolve(
    resolver: &Value,
    name: &[u8],
    context: &mut NameResolverContext,
) -> Result<Option<Vec<u8>>, bailout::Bailout> {
    let options = context.options.get();
    let cluster = context
        .private_data
        .as_ref()
        .and_then(|state| state.downcast_ref::<Owned>())
        .map(|cluster| cluster.copy());
    context.is_final = true;
    let cluster = if let Some(kept) = cluster {
        kept
    } else {
        context.release();
        context.cluster = false;
        bailout::outcome(())?;
        let argument = Owned::string(ZString::new(name));
        let found = resolver.call_method(None, b"lookup", &[&argument])?;
        if !swoole_rs_zend::host::exception().is_null() {
            return Ok(None);
        }
        match found.kind() {
            IS_STRING => {
                let host = found.bytes().unwrap_or_default();
                if host.is_empty() || family_of(host) == Some(options.0) {
                    return Ok(Some(host.to_vec()));
                }
                swoole_rs_zend::value_error(c"NameResolver::lookup() returned an invalid IP address for the requested family");
                return Ok(None);
            }
            IS_ARRAY => return Ok(node_address(&found, options)),
            IS_FALSE | IS_NULL => {
                context.is_final = false;
                return Ok(None);
            }
            IS_OBJECT
                if found.instance_of_named(&ZString::new(br"Swoole\NameResolver\Cluster"))?
                    == Some(true) =>
            {
                let Some(found) = found.clone_object()? else {
                    return Ok(None);
                };
                let kept = found.copy();
                context.private_data = Some(Box::new(found));
                context.cluster = true;
                kept
            }
            _ => {
                swoole_rs_zend::type_error(c"NameResolver::lookup() must return a Cluster, node array, IP string, false or null");
                return Ok(None);
            }
        }
    };
    loop {
        let node = cluster.call_method(None, b"pop", &[])?;
        if !swoole_rs_zend::host::exception().is_null() || node.kind() == IS_FALSE {
            return Ok(None);
        }
        let address = node_address(&node, options);
        if address.is_some() || !swoole_rs_zend::host::exception().is_null() {
            return Ok(address);
        }
    }
}

fn family_of(host: &[u8]) -> Option<libc::c_int> {
    let text = CString::new(host).ok()?;
    [libc::AF_INET, libc::AF_INET6]
        .into_iter()
        .find(|&family| swoole_rs_clients::address::verify_ip(family, &text))
}

fn node_address(node: &Value, (family, with_port): (libc::c_int, bool)) -> Option<Vec<u8>> {
    let Some(entry) = node.array() else {
        swoole_rs_zend::type_error(c"NameResolver cluster nodes must be arrays");
        return None;
    };
    let host = entry
        .find(b"host")
        .and_then(Value::bytes)
        .and_then(|host| Some((host, family_of(host)?)));
    let port = entry
        .find(b"port")
        .and_then(Value::as_long)
        .filter(|p| (1..=65535).contains(p));
    let (Some((host, host_family)), Some(port)) = (host, port) else {
        swoole_rs_zend::value_error(c"NameResolver node must contain a valid IP host and an integer port between 1 and 65535");
        return None;
    };
    if host_family != family {
        return None;
    }
    let mut result = host.to_vec();
    if with_port {
        result.push(if family == libc::AF_INET6 { b'@' } else { b':' });
        result.extend_from_slice(port.to_string().as_bytes());
    }
    Some(result)
}

swoole_rs_zend::handlers! {
    zim_swoole_name_resolver_context___construct => construct;
    zif_swoole_name_resolver_lookup => lookup;
    zif_swoole_name_resolver_add => add;
    zif_swoole_name_resolver_remove => remove;
}
