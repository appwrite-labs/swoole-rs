//! `Swoole\NameResolver\Context` and the `swoole_name_resolver_*` functions:
//! registration, objects, the functions' bodies and the PHP resolver's lookup
//! (a `Swoole\NameResolver` object's `lookup()`, then its cluster's `pop()`).
//! The context and the resolver list are crates/corebase's (`NameResolverContext`,
//! `swr_v1_core_name_resolver_*`), shared with the coroutine socket's lookup.
use crate::socket::warn;
pub use native::swoole_name_resolver_php_remove;
use std::{
    ffi::{c_char, c_int, c_void},
    mem, ptr,
};
use swoole_rs_phpstubs::name_resolver;
use swoole_rs_zend::{
    Call, Class, Object, Value, ZString, bailout, method_table,
    sys::{
        IS_OBJECT, IS_STRING, IS_UNDEF, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, swr_zend_efree,
        swr_zend_emalloc, swr_zend_runtime_instanceof, swr_zend_string_init,
        swr_zend_string_release, swr_zend_try_zval_dtor, swr_zend_zval_release, swr_zval_str,
        swr_zval_try_addref, zend_class_entry, zend_lookup_class, zend_object,
        zend_object_handlers, zval,
    },
    unset_property_deny, value,
};

/// `AF_INET`, `__construct`'s default family.
const AF_INET: i64 = libc::AF_INET as i64;
/// `SW_ZSTR_KNOWN(SW_ZEND_STR_CLASS_NAME_RESOLVER)`.
const RESOLVER_CLASS: &[u8] = b"Swoole\\NameResolver";

static CONTEXT: Class = Class::new();

/// A `NameResolver::Context` (crates/corebase's `NameResolverContext`) and the resolver
/// list, as the rest of this file uses them.
mod native {
    use std::ffi::{c_int, c_void};
    use swoole_rs_corebase::{NameResolverContext, TYPE_PHP};
    use swoole_rs_zend::{
        bailout,
        sys::{swr_zend_efree, swr_zend_try_zval_dtor, zval},
    };

    fn of(context: *mut c_void) -> *mut NameResolverContext {
        context.cast()
    }

    /// `new NameResolver::Context()`.
    pub(super) fn swoole_name_resolver_context_new() -> *mut c_void {
        Box::into_raw(Box::<NameResolverContext>::default()).cast()
    }

    /// `delete context`, running its destructor.
    /// # Safety
    /// `context` came from [`swoole_name_resolver_context_new`] and nothing else uses it.
    pub(super) unsafe fn swoole_name_resolver_context_free(context: *mut c_void) {
        // SAFETY: as the caller promises.
        drop(unsafe { Box::from_raw(of(context)) });
    }

    /// `*context = {}` with `with_port`: the old value is overwritten, not destroyed.
    pub(crate) fn swoole_name_resolver_context_reset(
        context: &mut NameResolverContext,
        with_port: bool,
    ) {
        let fresh = NameResolverContext {
            with_port,
            ..NameResolverContext::default()
        };
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
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream stores the long in an int"
        )]
        let kind = kind as c_int;
        context.kind = kind;
    }

    /// The cluster in `private_data` (null for none), and `with_port`.
    pub(super) fn swoole_name_resolver_context_get(
        context: &NameResolverContext,
        with_port: &mut bool,
    ) -> *mut zval {
        *with_port = context.with_port;
        context.private_data.cast()
    }

    /// The cluster's destructor: `zval_dtor` and `efree`.
    unsafe extern "C" fn free_cluster(context: &mut NameResolverContext) {
        let cluster = context.private_data.cast::<zval>();
        // SAFETY: the context's cluster, an emalloc'd zval.
        let bailed = unsafe { swr_zend_try_zval_dtor(cluster) };
        // SAFETY: as above, released.
        unsafe { swr_zend_efree(cluster.cast()) };
        bailout::defer(super::trapped(bailed));
    }

    /// With a cluster (an emalloc'd zval the context takes): keep it in `private_data`
    /// with its destructor, `cluster_` set and `final_` clear. Without: both flags as given.
    pub(super) fn swoole_name_resolver_context_settle(
        context: &mut NameResolverContext,
        cluster: *mut zval,
        is_final: bool,
    ) {
        if !cluster.is_null() {
            context.private_data = cluster.cast();
            context.dtor = Some(free_cluster);
        }
        context.cluster = !cluster.is_null();
        context.is_final = is_final;
    }

    /// `ctx->dtor(ctx); ctx->private_data = nullptr;`, a `bailout::trap` body (the
    /// cluster's destructor may run PHP).
    /// # Safety
    /// `context` is live with a cluster.
    pub(super) unsafe extern "C" fn swoole_name_resolver_context_release(context: *mut c_void) {
        // SAFETY: as the caller promises.
        unsafe { &mut *of(context) }.release();
    }

    /// `swoole_name_resolver_add({lookup, resolver, TYPE_PHP})`.
    pub(super) fn swoole_name_resolver_php_add(resolver: *mut zval) {
        swoole_rs_corebase::swr_v1_core_name_resolver_add(
            super::lookup_php,
            resolver.cast(),
            TYPE_PHP,
            true,
        );
    }

    /// A PHP resolver whose object is `resolver`'s, or any PHP resolver when it is null.
    unsafe extern "C" fn php_same(kept: *mut c_void, kind: c_int, resolver: *mut c_void) -> bool {
        // SAFETY: both are live object zvals when `resolver` is set.
        kind == TYPE_PHP
            && (resolver.is_null() || unsafe { super::same(kept.cast(), resolver.cast()) })
    }

    /// Remove the first PHP resolver for `resolver`'s object (any, when null) and return
    /// its kept zval, or null if there was none.
    /// # Safety
    /// `resolver` is null or a live object zval.
    pub unsafe fn swoole_name_resolver_php_remove(resolver: *mut zval) -> *mut zval {
        // SAFETY: `php_same` reads only the kept zvals and `resolver`.
        unsafe { swoole_rs_corebase::swr_v1_core_name_resolver_remove(php_same, resolver.cast()) }
            .cast()
    }
}

pub(crate) use native::swoole_name_resolver_context_reset;

/// `struct ContextObject { NameResolver::Context *context; zend_object std; }`.
type Held = *mut c_void;

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass of it.
    let std = unsafe { Object::<Held>::create(&CONTEXT, ce) };
    let context = native::swoole_name_resolver_context_new();
    // SAFETY: the object just made.
    unsafe { *Object::<Held>::data(std) = context };
    std
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    let context = unsafe { *Object::<Held>::data(std) };
    // SAFETY: the context `create` allocated.
    unsafe { native::swoole_name_resolver_context_free(context) };
    // SAFETY: as above.
    unsafe { Object::<Held>::dtor(std) };
}

/// `swoole_name_resolver_context_get_object_safe`: the object's context, or
/// Swoole's fatal error.
fn context_of(object: *mut zend_object) -> Option<*mut c_void> {
    // SAFETY: a live object; only ours carry this class's handlers.
    let ours = unsafe { (*object).handlers } == CONTEXT.handlers();
    let context = if ours {
        // SAFETY: one of our objects.
        unsafe { *Object::<Held>::data(object) }
    } else {
        ptr::null_mut()
    };
    if context.is_null() {
        // SAFETY: C++'s fatal-error report.
        unsafe { crate::native::unconstructed() };
        return None;
    }
    Some(context)
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_name_resolver_context___construct, &name_resolver::arginfo_class_Swoole_NameResolver_Context___construct, ZEND_ACC_PUBLIC;
    ];
}

/// MINIT: register `Swoole\NameResolver\Context`.
/// # Safety
/// Called once, at MINIT.
pub unsafe extern "C" fn php_swoole_name_resolver_minit(_module_number: c_int) {
    // SAFETY: MINIT, with a static table.
    unsafe {
        CONTEXT.register(
            c"Swoole\\NameResolver\\Context",
            METHODS.as_ptr(),
            configure,
        );
    }
}

/// Not serializable, clone and unset denied, custom object.
fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    Object::<Held>::install(entry, handlers, create, free);
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
    if let Some(context) = context_of(call.this()) {
        // SAFETY: the object's live context.
        native::swoole_name_resolver_context_set(
            unsafe { &mut *context.cast() },
            family,
            with_port,
        );
    }
}

/// `swoole_name_resolver_lookup(string $name, Swoole\NameResolver\Context $ctx): string`.
fn lookup(call: &Call) {
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.object(2)?)));
    let Ok((name, zcontext)) = parsed else {
        call.set_bool(false);
        return;
    };
    // SAFETY: an object argument.
    let Some(context) = context_of(unsafe { value::obj_of(zcontext) }) else {
        return;
    };
    let mut result = Vec::new();
    let mut run = swoole_rs_hooks::resolve::LookupCall {
        name: name.as_ptr().cast(),
        length: name.len(),
        context: context.cast(),
        out: (&raw mut result).cast(),
        assign: assign_vec,
    };
    // SAFETY: C++'s `swoole_name_resolver_lookup` over `run`; a PHP resolver
    // may bail out, which the trap catches.
    match unsafe {
        bailout::trap(
            swoole_rs_hooks::resolve::swoole_name_resolver_lookup_run,
            (&raw mut run).cast(),
        )
    } {
        Ok(()) => call.set_zstring(ZString::new(&result)),
        Err(bailout) => bailout.defer(),
    }
}

unsafe extern "C" fn assign_vec(out: *mut c_void, text: *const c_char, length: usize) {
    // SAFETY: `text` holds `length` bytes.
    let text = unsafe { std::slice::from_raw_parts(text.cast::<u8>(), length) };
    // SAFETY: `out` is the caller's `Vec<u8>`.
    unsafe { *out.cast::<Vec<u8>>() = text.to_vec() };
}

/// `swoole_name_resolver_add(Swoole\NameResolver $ns): bool`.
fn add(call: &Call) {
    match call.arity(1, 1).and_then(|()| call.object(1)) {
        // SAFETY: an object argument.
        Ok(resolver) => call.set_bool(unsafe { php_swoole_name_resolver_add(resolver) }),
        Err(_) => call.set_bool(false),
    }
}

/// `php_swoole_name_resolver_add`: register `resolver` (an object) if it is a
/// `Swoole\NameResolver`, holding a reference to it.
/// # Safety
/// `resolver` is a live object zval.
pub unsafe fn php_swoole_name_resolver_add(resolver: *mut zval) -> bool {
    // SAFETY: a request-allocated name, released below.
    let name =
        unsafe { swr_zend_string_init(RESOLVER_CLASS.as_ptr().cast(), RESOLVER_CLASS.len()) };
    // SAFETY: a live name; autoloads as upstream's known string does.
    let ce = unsafe { zend_lookup_class(name) };
    // SAFETY: our reference.
    unsafe { swr_zend_string_release(name) };
    let class = String::from_utf8_lossy(RESOLVER_CLASS);
    if ce.is_null() {
        warn(true, &format!("Class \"{class}\" not found"));
        return false;
    }
    // SAFETY: per the caller, an object.
    let object = unsafe { value::obj_of(resolver) };
    // SAFETY: a live object.
    let object_ce = unsafe { (*object).ce };
    // SAFETY: a live object's class and a registered class.
    if !unsafe { swr_zend_runtime_instanceof(object_ce, ce) } {
        warn(
            true,
            &format!("the given object is not an instance of {class}"),
        );
        return false;
    }
    // SAFETY: a live zval.
    unsafe { swr_zval_try_addref(resolver) };
    // SAFETY: Zend's request allocator (`sw_zval_dup`); the list owns the copy.
    let copy = unsafe { swr_zend_emalloc(mem::size_of::<zval>()) }.cast::<zval>();
    // SAFETY: a live zval.
    let held = unsafe { *resolver };
    // SAFETY: fresh storage for one zval.
    unsafe { copy.write(held) };
    // The list keeps the copy until remove or RSHUTDOWN.
    native::swoole_name_resolver_php_add(copy);
    true
}

/// `sw_php_spl_object_hash` equality: the same object handle.
unsafe extern "C" fn same(kept: *mut zval, resolver: *mut zval) -> bool {
    let handle = |zv: *mut zval| {
        // SAFETY: both are live object zvals.
        let object = unsafe { value::obj_of(zv) };
        // SAFETY: a live object.
        unsafe { (*object).handle }
    };
    handle(kept) == handle(resolver)
}

/// `swoole_name_resolver_remove(Swoole\NameResolver $ns): bool`.
fn remove(call: &Call) {
    let Ok(resolver) = call.arity(1, 1).and_then(|()| call.object(1)) else {
        call.set_bool(false);
        return;
    };
    // SAFETY: a live object zval.
    let kept = unsafe { native::swoole_name_resolver_php_remove(resolver) };
    if !kept.is_null() {
        // Drop the reference `add` took, through the argument as upstream does.
        // SAFETY: the argument holds another reference, so no destructor runs.
        bailout::defer(trapped(unsafe { swr_zend_try_zval_dtor(resolver) }));
        // SAFETY: the copy `add` allocated.
        unsafe { swr_zend_efree(kept.cast()) };
    }
    call.set_bool(!kept.is_null());
}

/// A shim's bailout flag.
fn trapped(flag: c_int) -> Result<(), bailout::Bailout> {
    if flag == 0 {
        Ok(())
    } else {
        Err(bailout::Bailout)
    }
}

/// A zval initialised as `IS_UNDEF`.
fn undef() -> zval {
    // SAFETY: all-zero bytes are an IS_UNDEF zval.
    let value: zval = unsafe { mem::zeroed() };
    debug_assert_eq!(IS_UNDEF, 0, "a zeroed zval is undefined");
    value
}

/// `method(arg)` on `object` into `retval`.
/// # Safety
/// `object` is a live object and `arg` null or a live zval.
unsafe fn call_method(
    object: *mut zval,
    method: &[u8],
    retval: &mut zval,
    arg: *mut zval,
) -> Result<(), bailout::Bailout> {
    // SAFETY: per the caller.
    let object = unsafe { value::obj_of(object) };
    let args = [arg];
    let args = if arg.is_null() { &[] } else { &args[..] };
    // SAFETY: as above; Zend writes `retval`.
    unsafe { bailout::call_method(object, ptr::null_mut(), method, retval, args) }
}

/// `php_swoole_name_resolver_lookup`: ask the `Swoole\NameResolver` `resolver`
/// for `name`, keeping a returned cluster in the context and popping a node
/// from it. Writes the result through `assign` and returns whether it is non-empty,
/// then raises a bailout the PHP code caused.
/// # Safety
/// `resolver` is a live object zval the list holds, `name` holds `length` bytes,
/// `context` is a live `NameResolver::Context *` and `assign` writes `out`.
unsafe extern "C" fn lookup_php(
    resolver: *mut c_void,
    name: *const c_char,
    length: usize,
    context: *mut swoole_rs_corebase::NameResolverContext,
    out: *mut c_void,
    assign: swoole_rs_corebase::Assign,
) -> bool {
    let (resolver, context) = (resolver.cast::<zval>(), context.cast::<c_void>());
    // SAFETY: per the caller.
    let name = unsafe { std::slice::from_raw_parts(name.cast::<u8>(), length) };
    // SAFETY: per the caller.
    let result = unsafe { resolve(resolver, name, context) };
    let answered = match result {
        Ok(Some(text)) => {
            // SAFETY: per the caller.
            unsafe { assign(out, text.as_ptr().cast(), text.len()) };
            !text.is_empty()
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
/// returns early from some steps without releasing `retval`; so does this.
/// # Safety
/// As [`lookup_php`].
unsafe fn resolve(
    resolver: *mut zval,
    name: &[u8],
    context: *mut c_void,
) -> Result<Option<Vec<u8>>, bailout::Bailout> {
    let mut with_port = false;
    // SAFETY: a live context.
    let mut cluster =
        native::swoole_name_resolver_context_get(unsafe { &*context.cast() }, &mut with_port);
    let mut retval = undef();
    if !cluster.is_null() {
        // SAFETY: the kept cluster object.
        unsafe { call_method(cluster, b"count", &mut retval, ptr::null_mut()) }?;
        // SAFETY: the call's result (IS_UNDEF if it threw).
        let count = unsafe { value::get_long(&raw const retval) };
        if bailout::pending() {
            return Ok(None);
        }
        if count == 0 {
            // No available node: release the cluster and resolve again.
            // SAFETY: the release of a live context with a cluster.
            unsafe { bailout::trap(native::swoole_name_resolver_context_release, context) }?;
            cluster = ptr::null_mut();
        }
    }
    if cluster.is_null() {
        let mut zname = undef();
        // SAFETY: a request-allocated copy of the name.
        let copy = unsafe { swr_zend_string_init(name.as_ptr().cast(), name.len()) };
        // SAFETY: `zname` owns the copy.
        unsafe { swr_zval_str(&raw mut zname, copy) };
        retval = undef();
        // SAFETY: the resolver object and a string argument.
        let called = unsafe { call_method(resolver, b"lookup", &mut retval, &raw mut zname) };
        called?;
        // SAFETY: `zname`'s string; strings run no destructors.
        unsafe { swr_zend_string_release(copy) };
        // SAFETY: the call's result, which `retval` holds.
        let found = unsafe { Value::from_ptr(&raw const retval) };
        match found.map_or(IS_UNDEF, Value::kind) {
            IS_OBJECT => {
                // SAFETY: Zend's request allocator (`ecalloc`), which the
                // context's destructor frees.
                let kept = unsafe { swr_zend_emalloc(mem::size_of::<zval>()) }.cast::<zval>();
                // SAFETY: fresh storage; the context takes `retval`'s reference.
                unsafe { kept.write(retval) };
                // SAFETY: a live context.
                native::swoole_name_resolver_context_settle(
                    unsafe { &mut *context.cast() },
                    kept,
                    false,
                );
                cluster = kept;
            }
            IS_STRING => {
                // SAFETY: a live context.
                native::swoole_name_resolver_context_settle(
                    unsafe { &mut *context.cast() },
                    ptr::null_mut(),
                    true,
                );
                return Ok(Some(
                    found.and_then(Value::bytes).unwrap_or_default().to_vec(),
                ));
            }
            _ => {
                // SAFETY: a live context.
                native::swoole_name_resolver_context_settle(
                    unsafe { &mut *context.cast() },
                    ptr::null_mut(),
                    false,
                );
                return Ok(None);
            }
        }
    }
    retval = undef();
    // SAFETY: the cluster object.
    unsafe { call_method(cluster, b"pop", &mut retval, ptr::null_mut()) }?;
    // SAFETY: the call's result, which `retval` holds until released below.
    let Some(node) = unsafe { Value::from_ptr(&raw const retval) }.and_then(Value::array) else {
        return Ok(None);
    };
    let Some(host) = node.find(b"host").and_then(Value::bytes) else {
        return Ok(None);
    };
    let mut result = host.to_vec();
    // The PHP calls may have re-run `__construct`: read `with_port` now, as upstream.
    // SAFETY: a live context.
    native::swoole_name_resolver_context_get(unsafe { &*context.cast() }, &mut with_port);
    if with_port {
        result.push(b':');
        let Some(port) = node.find(b"port").and_then(Value::applied_long) else {
            return Ok(None);
        };
        result.extend_from_slice(port.to_string().as_bytes());
    }
    // SAFETY: our reference to the popped node; releasing it may run PHP.
    unsafe { bailout::trap(swr_zend_zval_release, (&raw mut retval).cast()) }?;
    Ok(Some(result))
}

swoole_rs_zend::handlers! {
    zim_swoole_name_resolver_context___construct => construct;
    zif_swoole_name_resolver_lookup => lookup;
    zif_swoole_name_resolver_add => add;
    zif_swoole_name_resolver_remove => remove;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shim_flags_are_bailouts() {
        assert!(trapped(0).is_ok(), "0: the call returned");
        assert!(trapped(1).is_err(), "1: it bailed out");
    }
}
