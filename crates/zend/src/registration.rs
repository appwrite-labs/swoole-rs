//! Actual class and function alias construction, lookup and ownership.
use crate::{
    bailout::Bailout,
    string,
    sys::{
        self as z, HashTable, zend_class_entry, zend_function, zend_internal_arg_info, zend_string,
    },
};
use std::{
    ffi::{c_char, c_int},
    ptr,
};

fn leaf(code: c_int) -> Result<(), Bailout> {
    if code == 0 { Ok(()) } else { Err(Bailout) }
}
struct RequestString(*mut zend_string);
impl Drop for RequestString {
    fn drop(&mut self) {
        // SAFETY: this owner contains a live request string with no PHP destructor.
        unsafe { z::swr_zend_string_release(self.0) };
    }
}
struct RequestBytes(*mut c_char);
impl Drop for RequestBytes {
    fn drop(&mut self) {
        // SAFETY: this owner holds exactly one Zend request allocation.
        unsafe { z::swr_zend_efree(self.0.cast()) };
    }
}

/// Register `alias` in `target` using the named internal function's real handler
/// and argument metadata. Names are binary spans; lookup lowercases ASCII bytes.
/// # Errors
/// Returns a contained PHP registration bailout.
/// # Safety
/// All source spans and tables are live for this call. `info` is persistent
/// argument metadata. `assert_internal`, when present, is a non-PHP native
/// assertion leaf which must not unwind or bail out.
#[expect(
    clippy::too_many_arguments,
    reason = "preserves the SDK alias tables, binary names and native assertion effect"
)]
pub unsafe fn function_alias(
    source: *mut HashTable,
    origin: *const c_char,
    origin_length: usize,
    target: *mut HashTable,
    alias: *const c_char,
    alias_length: usize,
    info: *const zend_internal_arg_info,
    assert_internal: Option<unsafe extern "C" fn(u8)>,
) -> Result<c_int, Bailout> {
    // SAFETY: the caller lends the source name.
    let lower = RequestString(unsafe { z::swr_zend_string_init(origin, origin_length) });
    // SAFETY: this fresh request string is exclusively owned and has this exact span.
    unsafe { std::slice::from_raw_parts_mut(string::val(lower.0), origin_length) }
        .make_ascii_lowercase();
    // SAFETY: the source table and lowercase Zend string remain live for lookup.
    let slot = unsafe { z::zend_hash_find(source, lower.0) };
    drop(lower);
    if slot.is_null() {
        return Ok(-1);
    }
    // SAFETY: function tables store initialized pointer zvals.
    let value = unsafe { (*slot).value };
    // SAFETY: the table slot's union holds its zend_function pointer.
    let function = unsafe { value.ptr }.cast::<zend_function>();
    let kind = {
        // SAFETY: the source table owns this live function's common header.
        let entry = unsafe { &*function };
        // SAFETY: all function variants initialize their common header.
        let common = unsafe { entry.common };
        common.type_
    };
    if let Some(assert) = assert_internal {
        // SAFETY: the caller supplies a non-unwinding, non-PHP assertion leaf.
        // Preserve its original ordering before reading internal-only fields.
        unsafe { assert(kind) };
    }
    let (handler, args) = {
        // SAFETY: the caller's source function is internal, after its optional
        // native assertion, and remains live throughout this registration.
        let entry = unsafe { &*function };
        // SAFETY: this SDK operation's internal-function contract selects this union member.
        let internal = unsafe { &entry.internal_function };
        (internal.handler, internal.num_args)
    };
    // SAFETY: allocate the original independent request buffer, including its NUL.
    let name = RequestBytes(unsafe { z::swr_zend_emalloc(alias_length.wrapping_add(1)) }.cast());
    // SAFETY: the caller's readable alias span and this new independent allocation do not overlap.
    unsafe { ptr::copy_nonoverlapping(alias, name.0, alias_length) };
    // SAFETY: the allocation has space for the original trailing NUL.
    unsafe { name.0.wrapping_add(alias_length).write(0) };
    let mut entries = crate::zeroed::<[z::zend_function_entry; 2]>();
    entries[0].fname = name.0;
    entries[0].handler = handler;
    entries[0].arg_info = info;
    entries[0].num_args = args;
    let mut result = -1;
    // SAFETY: Zend borrows this complete terminated table and name while it copies
    // the registered metadata; its effects are contained below both request owners.
    leaf(unsafe {
        z::swr_zend_try_facade_register_functions(
            entries.as_ptr(),
            target,
            c_int::from(kind),
            &raw mut result,
        )
    })?;
    Ok(result)
}

/// Build the lowercase persistent interned class alias and register its real CE.
/// `sdk_prefix` retains the native SDK's handling of a leading backslash,
/// including its original string length; ordinary Rust aliases pass false.
/// # Errors
/// Returns a contained PHP interning or registration bailout.
/// # Safety
/// `name` is readable for `length` bytes, and `entry` is a live persistent CE.
pub unsafe fn class_alias(
    name: *const c_char,
    length: usize,
    entry: *mut zend_class_entry,
    sdk_prefix: bool,
) -> Result<c_int, Bailout> {
    // SAFETY: the source is live; PHP allocates the persistent owned name.
    let alias = unsafe { z::swr_zend_string_persistent(name, length) };
    // SAFETY: this persistent string is newly allocated and unshared.
    let bytes = unsafe { std::slice::from_raw_parts_mut(string::val(alias), length) };
    if sdk_prefix && bytes.first() == Some(&b'\\') {
        bytes.copy_within(1.., 0);
        if let Some(last) = bytes.last_mut() {
            *last = 0;
        }
    }
    bytes.make_ascii_lowercase();
    let mut interned = ptr::null_mut();
    // SAFETY: Zend consumes or interns the persistent string, as its native API
    // does; the successful result belongs to the process intern pool.
    leaf(unsafe { z::swr_zend_try_facade_intern(alias, &raw mut interned) })?;
    // SAFETY: successful interning returns a process-lived initialized string.
    let count = unsafe { (*interned).len };
    let mut result = -1;
    // SAFETY: the interned binary name and registered CE remain process-lived;
    // Zend copies the alias registration inside the contained leaf.
    leaf(unsafe {
        z::swr_zend_try_facade_class_alias(
            string::val(interned).cast(),
            count,
            entry,
            &raw mut result,
        )
    })?;
    Ok(result)
}
