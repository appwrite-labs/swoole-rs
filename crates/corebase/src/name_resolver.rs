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
}

impl Resolver {
    fn private_data(self) -> *mut c_void {
        core::ptr::with_exposed_provenance_mut(self.private_data)
    }
}

static RESOLVERS: Mutex<Vec<Resolver>> = Mutex::new(Vec::new());

fn resolvers() -> MutexGuard<'static, Vec<Resolver>> {
    RESOLVERS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `swoole_name_resolver_add({resolve, private_data, kind}, append)`.
pub fn swr_v1_core_name_resolver_add(
    resolve: Resolve,
    private_data: *mut c_void,
    kind: c_int,
    append: bool,
) {
    let resolver = Resolver {
        resolve,
        private_data: private_data.expose_provenance(),
        kind,
    };
    let mut list = resolvers();
    if append {
        list.push(resolver);
    } else {
        list.insert(0, resolver);
    }
}

/// Removes the first resolver `same(private_data, kind)` accepts and returns its
/// `private_data`, or null if none did. `same` runs under the list's lock.
pub fn swr_v1_core_name_resolver_remove(
    mut same: impl FnMut(*mut c_void, c_int) -> bool,
) -> *mut c_void {
    let mut list = resolvers();
    let found = (list.iter()).position(|r| same(r.private_data(), r.kind));
    let removed = found.map(|index| list.remove(index));
    drop(list);
    removed.map_or(core::ptr::null_mut(), Resolver::private_data)
}

/// `SwooleG = {}`'s effect on the list.
pub(crate) fn swr_v1_core_name_resolver_clear() {
    resolvers().clear();
}

/// Asks each resolver in turn; true once one answers (into `out`) or marks the context
/// final. False leaves the lookup to the system resolver.
///
/// A resolver may longjmp out (a PHP bailout), so this frame holds nothing to drop
/// across a call: each resolver is copied out under a lock released before the call.
pub fn swr_v1_core_name_resolver_resolve(
    name: &[u8],
    context: &mut NameResolverContext,
    out: &mut Vec<u8>,
) -> bool {
    let mut index = 0;
    loop {
        let next = resolvers().get(index).copied();
        let Some(resolver) = next else {
            return false;
        };
        index += 1;
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
        let of_kind = |_, kind| kind == 2;
        assert!(!swr_v1_core_name_resolver_remove(of_kind).is_null());
        assert_eq!(lookup("name1"), (false, Vec::new()));
        assert!(swr_v1_core_name_resolver_remove(of_kind).is_null());
    }
}
