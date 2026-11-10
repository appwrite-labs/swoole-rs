//! Upstream's `SwooleG.name_resolvers`: the resolvers `swoole_name_resolver_lookup` asks
//! in turn before the system lookup, and `NameResolver::Context`, a lookup's state.
//! The list is process-wide, as `SwooleG` is; `SwooleG = {}` clears it.
use core::ffi::{c_char, c_int, c_void};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// `NameResolver::Context`; native code asserts the same layout.
#[repr(C)]
pub struct NameResolverContext {
    pub kind: c_int,
    pub timeout: f64,
    pub private_data: *mut c_void,
    pub with_port: bool,
    pub cluster: bool,
    pub is_final: bool,
    /// Frees `private_data`; the context's destructor runs it when both are set.
    pub dtor: Option<unsafe extern "C" fn(&mut NameResolverContext)>,
}

impl Default for NameResolverContext {
    fn default() -> Self {
        Self {
            kind: 0,
            timeout: 0.0,
            private_data: core::ptr::null_mut(),
            with_port: false,
            cluster: false,
            is_final: false,
            dtor: None,
        }
    }
}

impl NameResolverContext {
    /// `ctx->dtor(ctx); ctx->private_data = nullptr;` when there is something to free.
    pub fn release(&mut self) {
        if let Some(dtor) = self.dtor
            && !self.private_data.is_null()
        {
            // SAFETY: the destructor whoever set `private_data` installed for it.
            unsafe { dtor(self) };
        }
        self.private_data = core::ptr::null_mut();
    }
}

impl Drop for NameResolverContext {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<NameResolverContext>() == 40
        && core::mem::offset_of!(NameResolverContext, is_final) == 26
        && core::mem::offset_of!(NameResolverContext, dtor) == 32,
    "NameResolver::Context's layout moved"
);

/// Where a resolver writes its answer: `assign(out, text, length)`.
pub type Assign = unsafe extern "C" fn(out: *mut c_void, text: *const c_char, length: usize);

/// A resolver: answers `name` for `context` through `assign` and returns true, or returns
/// false with nothing written. It may set `context.is_final` to end the lookup.
pub type Resolve = unsafe extern "C" fn(
    private_data: *mut c_void,
    name: *const c_char,
    length: usize,
    context: *mut NameResolverContext,
    out: *mut c_void,
    assign: Assign,
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
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_name_resolver_add(
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

/// Removes the first resolver `same(private_data, kind, argument)` accepts and returns its
/// `private_data`, or null if none did.
/// # Safety
/// `same` is safe to call with each listed resolver and `argument`, and does not touch the list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_name_resolver_remove(
    same: unsafe extern "C" fn(*mut c_void, c_int, *mut c_void) -> bool,
    argument: *mut c_void,
) -> *mut c_void {
    let mut list = resolvers();
    // SAFETY: the caller's predicate over a listed resolver's own fields.
    let found = (list.iter()).position(|r| unsafe { same(r.private_data(), r.kind, argument) });
    found.map_or(core::ptr::null_mut(), |index| {
        list.remove(index).private_data()
    })
}

/// `SwooleG = {}`'s effect on the list.
pub(crate) fn swr_v1_core_name_resolver_clear() {
    resolvers().clear();
}

/// Asks each resolver in turn; true once one answers (through `assign`) or marks the
/// context final. False leaves the lookup to the system resolver.
///
/// A resolver may longjmp out (a PHP bailout), so this frame holds nothing to drop
/// across a call: each resolver is copied out under a lock released before the call.
///
/// # Safety
/// `name` holds `length` bytes and `assign` writes `out`.
pub unsafe fn swr_v1_core_name_resolver_resolve(
    name: *const c_char,
    length: usize,
    context: &mut NameResolverContext,
    out: *mut c_void,
    assign: Assign,
) -> bool {
    let mut index = 0;
    loop {
        let next = resolvers().get(index).copied();
        let Some(resolver) = next else {
            return false;
        };
        index += 1;
        // SAFETY: a registered resolver over the caller's live arguments.
        let answered = unsafe {
            (resolver.resolve)(
                resolver.private_data(),
                name,
                length,
                &raw mut *context,
                out,
                assign,
            )
        };
        if answered || context.is_final {
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn answer(
        private_data: *mut c_void,
        name: *const c_char,
        length: usize,
        context: *mut NameResolverContext,
        out: *mut c_void,
        assign: Assign,
    ) -> bool {
        let name = unsafe { core::slice::from_raw_parts(name.cast::<u8>(), length) };
        match name {
            b"name1" => unsafe { assign(out, private_data.cast(), 9) },
            b"stop" => unsafe { (*context).is_final = true },
            _ => return false,
        }
        name == b"name1"
    }

    unsafe extern "C" fn keep(out: *mut c_void, text: *const c_char, length: usize) {
        let text = unsafe { core::slice::from_raw_parts(text.cast::<u8>(), length) };
        unsafe { *out.cast::<Vec<u8>>() = text.to_vec() };
    }

    unsafe extern "C" fn of_kind(_: *mut c_void, kind: c_int, wanted: *mut c_void) -> bool {
        kind == unsafe { *wanted.cast::<c_int>() }
    }

    fn lookup(name: &str) -> (bool, Vec<u8>) {
        let mut context = NameResolverContext::default();
        let mut out = Vec::new();
        let found = unsafe {
            swr_v1_core_name_resolver_resolve(
                name.as_ptr().cast(),
                name.len(),
                &mut context,
                (&raw mut out).cast(),
                keep,
            )
        };
        (found, out)
    }

    #[test]
    fn resolvers_answer_in_turn_until_removed() {
        swr_v1_core_name_resolver_clear();
        swr_v1_core_name_resolver_add(answer, c"127.0.0.2".as_ptr().cast_mut().cast(), 2, true);
        assert_eq!(lookup("name1"), (true, b"127.0.0.2".to_vec()));
        assert_eq!(lookup("stop"), (true, Vec::new()));
        assert_eq!(lookup("localhost"), (false, Vec::new()));
        let mut user: c_int = 2;
        let user = (&raw mut user).cast();
        assert!(!unsafe { swr_v1_core_name_resolver_remove(of_kind, user) }.is_null());
        assert_eq!(lookup("name1"), (false, Vec::new()));
        assert!(unsafe { swr_v1_core_name_resolver_remove(of_kind, user) }.is_null());
    }
}
