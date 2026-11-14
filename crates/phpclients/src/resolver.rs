//! `Swoole\NameResolver\Context` and the `swoole_name_resolver_*` functions:
//! registration, objects, the functions' bodies and the PHP resolver's lookup
//! (a `Swoole\NameResolver` object's `lookup()`, then its cluster's `pop()`).
//! The context and the resolver list are crates/corebase's (`NameResolverContext`,
//! `swr_v1_core_name_resolver_*`), shared with the coroutine socket's lookup; a PHP
//! resolver's entry names its object, whose reference crates/phpcoroutines keeps.
use crate::socket::warn;
pub use native::swoole_name_resolver_php_remove;
use std::{cell::UnsafeCell, ffi::c_void, mem::ManuallyDrop};
use swoole_rs_corebase::NameResolverContext;
use swoole_rs_phpstubs::name_resolver;
use swoole_rs_zend::{
    Call, Custom, CustomClass, Minit, Owned, Value, ZString, bailout, method_table,
    sys::{IS_OBJECT, IS_STRING, ZEND_ACC_PUBLIC},
};

/// `AF_INET`, `__construct`'s default family.
const AF_INET: i64 = libc::AF_INET as i64;
/// `SW_ZSTR_KNOWN(SW_ZEND_STR_CLASS_NAME_RESOLVER)`.
const RESOLVER_CLASS: &[u8] = b"Swoole\\NameResolver";

static CONTEXT: CustomClass<Held> = CustomClass::new();

/// A `NameResolver::Context` (crates/corebase's `NameResolverContext`) and the resolver
/// list, as the rest of this file uses them.
mod native {
    use std::ffi::{c_int, c_void};
    use swoole_rs_corebase::CCast;
    use swoole_rs_corebase::{NameResolverContext, TYPE_PHP};
    use swoole_rs_phpcoroutines::name_resolvers;
    use swoole_rs_zend::{Owned, Value};

    /// `*context = {}` with `with_port`: the old value is overwritten, not destroyed.
    pub(crate) fn swoole_name_resolver_context_reset(
        context: &mut NameResolverContext,
        with_port: bool,
    ) {
        let fresh = NameResolverContext {
            with_port,
            ..NameResolverContext::default()
        };
        #[expect(
            clippy::mem_forget,
            reason = "upstream overwrites the old context without releasing it"
        )]
        std::mem::forget(std::mem::replace(context, fresh));
    }

    /// `__construct`'s `type` and `with_port`.
    pub(super) fn swoole_name_resolver_context_set(
        context: &mut NameResolverContext,
        kind: i64,
        with_port: bool,
    ) {
        context.with_port = with_port;
        // `int` storage, as upstream's assignment.
        let kind = kind.c_cast::<c_int>();
        context.kind = kind;
    }

    /// A reference of our own to the cluster in `private_data`, if any: the PHP calls
    /// on it may reach the context.
    pub(super) fn swoole_name_resolver_context_cluster(
        context: &NameResolverContext,
    ) -> Option<Owned> {
        let cluster = context.private_data.as_ref()?.downcast_ref::<Owned>()?;
        Some(cluster.copy())
    }

    /// With a cluster: keep it in `private_data` (dropped with the context), `cluster_`
    /// set and `final_` clear. Without: both flags as given.
    pub(super) fn swoole_name_resolver_context_settle(
        context: &mut NameResolverContext,
        cluster: Option<Owned>,
        is_final: bool,
    ) {
        context.cluster = cluster.is_some();
        if let Some(cluster) = cluster {
            context.private_data = Some(Box::new(cluster));
        }
        context.is_final = is_final;
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

/// `struct ContextObject { NameResolver::Context *context; zend_object std; }`. The
/// context is boxed and reached through its pointer: a lookup keeps using it across
/// PHP code, which may run `__construct` on the same object.
struct Held(Box<UnsafeCell<NameResolverContext>>);

impl Custom for Held {
    fn class() -> &'static CustomClass<Self> {
        &CONTEXT
    }
    fn new() -> Self {
        Self(Box::default())
    }
}

/// `swoole_name_resolver_context_get_object_safe`: the object's context, or
/// Swoole's fatal error.
fn context_of(held: Option<&Held>) -> Option<*mut NameResolverContext> {
    let context = held.map(|held| held.0.get());
    if context.is_none() {
        crate::native::unconstructed();
    }
    context
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
    minit.register_object::<Held>(c"Swoole\\NameResolver\\Context", METHODS.rows(), 0);
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
    if let Some(context) = context_of(call.custom::<Held>()) {
        // SAFETY: the object's live context.
        native::swoole_name_resolver_context_set(unsafe { &mut *context }, family, with_port);
    }
}

/// `swoole_name_resolver_lookup(string $name, Swoole\NameResolver\Context $ctx): string`.
fn lookup(call: &Call) {
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.object(2)?)));
    let Ok((name, _)) = parsed else {
        call.set_bool(false);
        return;
    };
    let held = call.value(2).map(Value::dereferenced);
    let Some(context) = context_of(held.as_deref().and_then(Value::custom::<Held>)) else {
        return;
    };
    // SAFETY: `context_of`'s live `NameResolver::Context`, which this call borrows alone.
    let context = unsafe { &mut *context };
    let mut result = Vec::new();
    // The lookup holds only borrows of this frame; a PHP resolver may bail out,
    // which the trap catches.
    let lookup = || swoole_rs_hooks::resolve::name_resolver_lookup(name, context, &mut result);
    match swoole_rs_zend::contain_with(lookup) {
        Ok(()) => call.set_zstring(ZString::new(&result)),
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

/// `method(arg)` on the object `object` holds: the result (`IS_UNDEF` if it
/// threw). Upstream leaves the result of a call that bailed out, so does this.
fn call_method(
    object: &Value,
    method: &[u8],
    arg: Option<&Value>,
) -> Result<ManuallyDrop<Owned>, bailout::Bailout> {
    object
        .call_method(None, method, arg.as_slice())
        .map(ManuallyDrop::new)
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
    bailout::finish();
    answered
}

/// The lookup's steps, as upstream's; `None` is an empty result. Upstream
/// returns early from some steps without releasing a call's result; so does
/// this ([`call_method`]'s `ManuallyDrop`).
fn resolve(
    resolver: &Value,
    name: &[u8],
    context: &mut NameResolverContext,
) -> Result<Option<Vec<u8>>, bailout::Bailout> {
    let mut cluster = native::swoole_name_resolver_context_cluster(context);
    if let Some(kept) = &cluster {
        let count = call_method(kept, b"count", None)?.long();
        if bailout::pending() {
            return Ok(None);
        }
        if count == 0 {
            // No available node: release the cluster and resolve again.
            cluster = None;
            // The context's cluster is an `Owned`, whose drop releases it inside
            // `zend_try` and defers a destructor's bailout.
            context.release();
            bailout::trapped(0)?;
        }
    }
    let cluster = if let Some(cluster) = cluster {
        cluster
    } else {
        let zname = Owned::string(ZString::new(name));
        let found = call_method(resolver, b"lookup", Some(&zname))?;
        let kind = found.kind();
        let answer = (kind == IS_STRING).then(|| found.bytes().unwrap_or_default().to_vec());
        // The context takes an object result's reference; any other is left, as upstream.
        let found = (kind == IS_OBJECT).then(|| ManuallyDrop::into_inner(found));
        let kept = found.as_ref().map(|found| found.copy());
        native::swoole_name_resolver_context_settle(context, found, kind == IS_STRING);
        match (kind, kept) {
            (IS_OBJECT, Some(kept)) => kept,
            (IS_STRING, _) => return Ok(answer),
            _ => return Ok(None),
        }
    };
    let node = call_method(&cluster, b"pop", None)?;
    let Some(entry) = node.array() else {
        return Ok(None);
    };
    let Some(host) = entry.find(b"host").and_then(Value::bytes) else {
        return Ok(None);
    };
    let mut result = host.to_vec();
    // The PHP calls may have re-run `__construct`: read `with_port` now, as upstream.
    if context.with_port {
        result.push(b':');
        let Some(port) = entry.find(b"port").and_then(Value::applied_long) else {
            return Ok(None);
        };
        result.extend_from_slice(port.to_string().as_bytes());
    }
    // Our reference to the popped node, released inside `zend_try`: a destructor's
    // bailout stays pending, and stops the lookup.
    drop(ManuallyDrop::into_inner(node));
    bailout::outcome(Some(result))
}

swoole_rs_zend::handlers! {
    zim_swoole_name_resolver_context___construct => construct;
    zif_swoole_name_resolver_lookup => lookup;
    zif_swoole_name_resolver_add => add;
    zif_swoole_name_resolver_remove => remove;
}
