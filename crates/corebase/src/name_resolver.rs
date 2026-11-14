//! Upstream's `SwooleG.name_resolvers`: the resolvers `swoole_name_resolver_lookup` asks
//! in turn before the system lookup, and `NameResolver::Context`, a lookup's state.
//! The list is process-wide, as `SwooleG` is; `SwooleG = {}` clears it.
use core::any::Any;
use core::ffi::{c_int, c_void};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// `NameResolver::Context`. Native code only holds a pointer to one.
#[derive(Default)]
pub struct NameResolverContext {
    pub kind: c_int,
    pub timeout: f64,
    /// A resolver's state kept for the rest of the lookup (the PHP resolver's
    /// cluster), dropped with the context or by [`release`](Self::release).
    pub private_data: Option<Box<dyn Any>>,
    pub with_port: bool,
    pub cluster: bool,
    pub is_final: bool,
}

impl NameResolverContext {
    /// `ctx->dtor(ctx); ctx->private_data = nullptr;`.
    pub fn release(&mut self) {
        self.private_data = None;
    }
}

/// A resolver: answers `name` for `context` into `out` and returns true, or returns
/// false with nothing written. It may set `context.is_final` to end the lookup.
/// `private_data` is what was registered with it.
pub type Resolve = fn(
    private_data: *mut c_void,
    name: &[u8],
    context: &mut NameResolverContext,
    out: &mut Vec<u8>,
) -> bool;

/// `NameResolver::Type`'s `TYPE_PHP`.
pub const TYPE_PHP: c_int = 1;

/// `NameResolver`; `private_data` is an exposed address, so the list is `Send`.
#[derive(Clone, Copy)]
struct Resolver {
    resolve: Resolve,
    private_data: usize,
    kind: c_int,
    /// Its place in the list: ids rise along it, and are never reused.
    id: i64,
}

impl Resolver {
    fn private_data(self) -> *mut c_void {
        core::ptr::with_exposed_provenance_mut(self.private_data)
    }
}

/// The resolvers, and the ids the next prepended and appended ones take.
struct List {
    entries: Vec<Resolver>,
    front: i64,
    back: i64,
}

static RESOLVERS: Mutex<List> = Mutex::new(List {
    entries: Vec::new(),
    front: 0,
    back: 1,
});

fn resolvers() -> MutexGuard<'static, List> {
    RESOLVERS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `swoole_name_resolver_add({resolve, private_data, kind}, append)`.
pub fn swr_v1_core_name_resolver_add(
    resolve: Resolve,
    private_data: *mut c_void,
    kind: c_int,
    append: bool,
) {
    let mut list = resolvers();
    let id = if append { list.back } else { list.front };
    let resolver = Resolver {
        resolve,
        private_data: private_data.expose_provenance(),
        kind,
        id,
    };
    if append {
        list.back += 1;
        list.entries.push(resolver);
    } else {
        list.front -= 1;
        list.entries.insert(0, resolver);
    }
}

/// Removes the first resolver `same(private_data, kind)` accepts and returns its
/// `private_data`, or null if none did. `same` runs under the list's lock.
pub fn swr_v1_core_name_resolver_remove(
    mut same: impl FnMut(*mut c_void, c_int) -> bool,
) -> *mut c_void {
    let mut list = resolvers();
    let found = (list.entries.iter()).position(|r| same(r.private_data(), r.kind));
    let removed = found.map(|index| list.entries.remove(index));
    drop(list);
    removed.map_or(core::ptr::null_mut(), Resolver::private_data)
}

/// `SwooleG = {}`'s effect on the list.
pub(crate) fn swr_v1_core_name_resolver_clear() {
    resolvers().entries.clear();
}

/// Asks each resolver in turn; true once one answers (into `out`) or marks the context
/// final. False leaves the lookup to the system resolver.
///
/// A resolver's code may add or remove resolvers, so each step asks the first one
/// after the last asked, as upstream's list iterator does: one appended meanwhile is
/// asked, one prepended or removed is not. A resolver may longjmp out (a PHP
/// bailout), so this frame holds nothing to drop across a call, and the lock is
/// released before it.
pub fn swr_v1_core_name_resolver_resolve(
    name: &[u8],
    context: &mut NameResolverContext,
    out: &mut Vec<u8>,
) -> bool {
    let mut last = i64::MIN;
    loop {
        let next = resolvers().entries.iter().find(|r| r.id > last).copied();
        let Some(resolver) = next else {
            return false;
        };
        last = resolver.id;
        if (resolver.resolve)(resolver.private_data(), name, context, out) || context.is_final {
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(
        private_data: *mut c_void,
        name: &[u8],
        context: &mut NameResolverContext,
        out: &mut Vec<u8>,
    ) -> bool {
        match name {
            b"name1" => {
                let text = unsafe { core::ffi::CStr::from_ptr(private_data.cast()) };
                *out = text.to_bytes().to_vec();
            }
            b"stop" => context.is_final = true,
            _ => return false,
        }
        name == b"name1"
    }

    /// How many times [`leave`] was asked.
    static LEFT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

    fn leave(_: *mut c_void, _: &[u8], _: &mut NameResolverContext, _: &mut Vec<u8>) -> bool {
        LEFT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        swr_v1_core_name_resolver_remove(|_, kind| kind == 3);
        false
    }

    fn join(_: *mut c_void, _: &[u8], _: &mut NameResolverContext, _: &mut Vec<u8>) -> bool {
        swr_v1_core_name_resolver_add(answer, c"127.0.0.3".as_ptr().cast_mut().cast(), 5, true);
        false
    }

    fn lookup(name: &str) -> (bool, Vec<u8>) {
        let mut context = NameResolverContext::default();
        let mut out = Vec::new();
        let found = swr_v1_core_name_resolver_resolve(name.as_bytes(), &mut context, &mut out);
        (found, out)
    }

    #[test]
    fn resolvers_answer_in_turn_until_removed() {
        swr_v1_core_name_resolver_clear();
        swr_v1_core_name_resolver_add(answer, c"127.0.0.2".as_ptr().cast_mut().cast(), 2, true);
        assert_eq!(lookup("name1"), (true, b"127.0.0.2".to_vec()));
        assert_eq!(lookup("stop"), (true, Vec::new()));
        assert_eq!(lookup("localhost"), (false, Vec::new()));
        // One that unlinks itself mid-lookup: the next is still asked.
        swr_v1_core_name_resolver_add(leave, core::ptr::null_mut(), 3, false);
        assert_eq!(lookup("name1"), (true, b"127.0.0.2".to_vec()));
        assert!(swr_v1_core_name_resolver_remove(|_, kind| kind == 3).is_null());
        // Two identical entries, the first unlinking one of them: both are still asked.
        LEFT.store(0, core::sync::atomic::Ordering::Relaxed);
        swr_v1_core_name_resolver_add(leave, core::ptr::null_mut(), 3, false);
        swr_v1_core_name_resolver_add(leave, core::ptr::null_mut(), 3, false);
        assert_eq!(lookup("name1"), (true, b"127.0.0.2".to_vec()));
        assert_eq!(LEFT.load(core::sync::atomic::Ordering::Relaxed), 2);
        assert!(swr_v1_core_name_resolver_remove(|_, kind| kind == 3).is_null());
        let of_kind = |_, kind| kind == 2;
        assert!(!swr_v1_core_name_resolver_remove(of_kind).is_null());
        assert_eq!(lookup("name1"), (false, Vec::new()));
        assert!(swr_v1_core_name_resolver_remove(of_kind).is_null());
        // One appended mid-lookup is asked in the same lookup.
        swr_v1_core_name_resolver_add(join, c"join".as_ptr().cast_mut().cast(), 4, true);
        assert_eq!(lookup("name1"), (true, b"127.0.0.3".to_vec()));
        assert!(!swr_v1_core_name_resolver_remove(|_, kind| kind == 4).is_null());
        assert!(!swr_v1_core_name_resolver_remove(|_, kind| kind == 5).is_null());
    }
}
