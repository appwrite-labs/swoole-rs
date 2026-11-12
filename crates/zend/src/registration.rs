//! Actual class and function alias construction, lookup and ownership.
use crate::{
    Value, ZString,
    bailout::Bailout,
    string,
    sys::{self as z, HashTable, zend_class_entry, zend_function, zend_internal_arg_info},
};
use std::{
    ffi::{CStr, c_char, c_int},
    ptr,
};

fn leaf(code: c_int) -> Result<(), Bailout> {
    if code == 0 { Ok(()) } else { Err(Bailout) }
}

/// Register `alias` in `target` using the internal function `origin` of
/// `source`: its real handler and argument count, with `info`. Lookup
/// lowercases `origin`'s ASCII bytes; -1 when `source` has no such function.
/// # Errors
/// Returns a contained PHP registration bailout.
/// # Safety
/// Both tables are live function tables for this call, `source`'s `origin` is
/// an internal function, and `info` is persistent argument metadata for it.
pub unsafe fn function_alias(
    source: *mut HashTable,
    origin: &CStr,
    target: *mut HashTable,
    alias: &CStr,
    info: *const zend_internal_arg_info,
) -> Result<c_int, Bailout> {
    let lower = ZString::new(&origin.to_bytes().to_ascii_lowercase());
    // SAFETY: the source table and lowercase Zend string remain live for lookup.
    let slot = unsafe { z::zend_hash_find(source, lower.as_ptr()) };
    drop(lower);
    // SAFETY: null, or a slot of the caller's live table.
    let Some(function) = unsafe { Value::from_ptr(slot) }.and_then(Value::pointer) else {
        return Ok(-1);
    };
    // SAFETY: the source table owns this live function, which remains live and
    // unchanged throughout this registration.
    let entry = unsafe { &*function.cast::<zend_function>() };
    let kind = {
        // SAFETY: all function variants initialize their common header.
        let common = unsafe { entry.common };
        common.type_
    };
    let (handler, args) = {
        // SAFETY: the caller's source function is internal: this operation's
        // contract selects this union member.
        let internal = unsafe { &entry.internal_function };
        (internal.handler, internal.num_args)
    };
    let mut entries = crate::zeroed::<[z::zend_function_entry; 2]>();
    entries[0].fname = alias.as_ptr();
    entries[0].handler = handler;
    entries[0].arg_info = info;
    entries[0].num_args = args;
    let mut result = -1;
    // SAFETY: Zend borrows this complete terminated table and the NUL-terminated
    // name while it copies the registered metadata, inside the contained leaf.
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
