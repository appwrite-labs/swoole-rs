//! Shared object/property and callback custody operations for native PHP callers.
use std::{
    ffi::{c_char, c_int},
    ptr,
};
use swoole_rs_zend::{
    bailout, string,
    sys::{
        self, zend_class_entry, zend_fcall_info, zend_fcall_info_cache, zend_object,
        zend_object_handlers, zend_string, zval,
    },
    type_of,
};

fn bailed(code: c_int) -> bool {
    if code != 0 {
        bailout::Bailout.defer();
        true
    } else {
        false
    }
}

unsafe fn destroy(value: *mut zval) {
    // SAFETY: the live value's destructor executes entirely within the C trap.
    bailout::defer(unsafe { bailout::trap(sys::swr_zend_zval_release, value.cast()) });
}

/// # Safety
/// Return uninitialized request memory for one zval; callers must initialize it before use.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_zval_alloc() -> *mut zval {
    // SAFETY: Zend's request allocator returns suitably aligned storage.
    let value = unsafe { super::owned::alloc_request(size_of::<zval>()) }.cast();
    bailout::finish();
    value
}

/// # Safety
/// A live source zval. This intentionally copies only the slot, without retaining its payload.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_zval_dup(value: *const zval) -> *mut zval {
    // SAFETY: one fresh request allocation, not a reference-count operation.
    let dest = unsafe { super::owned::alloc_request(size_of::<zval>()) }.cast::<zval>();
    bailout::finish();
    // SAFETY: the source is initialized and independent of the allocation.
    let value = unsafe { value.read() };
    // SAFETY: the destination has space for one initialized slot.
    unsafe { dest.write(value) };
    dest
}

/// # Safety
/// `value` is a live request-allocated zval, consumed once.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_zval_free(value: *mut zval) {
    // SAFETY: release the value before its containing allocation, as upstream does.
    unsafe { destroy(value) };
    if !bailout::pending() {
        // SAFETY: the caller owns this Zend request allocation.
        unsafe { sys::swr_zend_efree(value.cast()) };
    }
    bailout::finish();
}

/// # Safety
/// A non-interned request string with capacity `allocated`; `length` is within that capacity.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_string_recycle(
    mut value: *mut zend_string,
    allocated: usize,
    length: usize,
    page_size: usize,
) -> *mut zend_string {
    if allocated != length {
        if allocated > page_size && allocated > length.wrapping_mul(2) {
            // SAFETY: a non-interned, exclusively owned request string.
            let mut resized = ptr::null_mut();
            // SAFETY: the request string's realloc executes in the C trap.
            bailed(unsafe { sys::swr_zend_try_facade_realloc(value, length, &raw mut resized) });
            bailout::finish();
            value = resized;
        } else {
            // SAFETY: length does not exceed the original allocation.
            unsafe { (*value).len = length };
        }
    }
    // SAFETY: the current allocation contains length bytes and their trailing NUL.
    unsafe { string::val(value).wrapping_add(length).write(0) };
    value
}

/// # Safety
/// Registered class and stable handlers, called by Zend's object factory.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_object_new(
    class: *mut zend_class_entry,
    handlers: *const zend_object_handlers,
) -> *mut zend_object {
    // SAFETY: Zend allocates an object and its class's property slots.
    let mut allocation = ptr::null_mut();
    // SAFETY: trap allocation failure before initializing any Rust-visible object fields.
    bailed(unsafe {
        sys::swr_zend_try_facade_object_alloc(size_of::<zend_object>(), class, &raw mut allocation)
    });
    bailout::finish();
    let object = allocation.cast::<zend_object>();
    // SAFETY: fresh storage and a registered class.
    unsafe { sys::zend_object_std_init(object, class) };
    // SAFETY: standard initialization created the object's property table.
    unsafe { sys::object_properties_init(object, class) };
    // SAFETY: the fresh object borrows its class's process-lived handlers.
    unsafe { (*object).handlers = handlers };
    object
}

unsafe fn read(
    scope: *mut zend_class_entry,
    object: *mut zend_object,
    name: *const c_char,
    length: usize,
    silent: bool,
    rv: *mut zval,
) -> *mut zval {
    let mut result = ptr::null_mut();
    // SAFETY: live scope/object/name and writable result/temporary slots.
    bailed(unsafe {
        sys::swr_zend_try_facade_read(scope, object, name, length, silent, rv, &raw mut result)
    });
    result
}

unsafe fn array_property(
    scope: *mut zend_class_entry,
    object: *mut zend_object,
    name: *const c_char,
    length: usize,
) -> *mut zval {
    // SAFETY: a zeroed slot is a valid undefined zval.
    let mut value: zval = unsafe { std::mem::zeroed() };
    // SAFETY: construct a request array in an initialized writable slot.
    if bailed(unsafe { sys::swr_zend_try_facade_array_init(&raw mut value) }) {
        return ptr::null_mut();
    }
    // SAFETY: update_property can execute PHP, which the C shim traps.
    bailed(unsafe { sys::swr_zend_try_facade_update(scope, object, name, length, &raw mut value) });
    if bailout::pending() {
        return ptr::null_mut();
    }
    // SAFETY: release the temporary after the object has taken its reference.
    unsafe { destroy(&raw mut value) };
    if bailout::pending() {
        return ptr::null_mut();
    }
    // SAFETY: the object owns this declared property; its result is not the local rv.
    unsafe { read(scope, object, name, length, true, &raw mut value) }
}

/// # Safety
/// Live internal scope/object with a declared property; returned storage belongs to the object.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_property(
    scope: *mut zend_class_entry,
    value: *mut zval,
    name: *const c_char,
    length: usize,
    silent: bool,
    array: bool,
) -> *mut zval {
    // SAFETY: the source holds a live object.
    let object = unsafe { swoole_rs_zend::value::obj_of(value) };
    // SAFETY: initialize Zend's scratch return slot.
    let mut temporary: zval = unsafe { std::mem::zeroed() };
    // SAFETY: caller guarantees an object-owned declared property result.
    let mut property = unsafe { read(scope, object, name, length, silent, &raw mut temporary) };
    if !bailout::pending() {
        // SAFETY: Zend's distinguished uninitialized-value address is process/thread global.
        let undefined = unsafe { sys::swr_zend_uninitialized_zval() };
        if array {
            // SAFETY: a successful property read returns a live zval.
            if unsafe { type_of(property) } != sys::IS_ARRAY {
                if property == undefined {
                    // SAFETY: construct and publish the missing array property.
                    property = unsafe { array_property(scope, object, name, length) };
                } else {
                    // SAFETY: replace the existing value in the object's property slot.
                    unsafe { destroy(property) };
                    if !bailout::pending() {
                        // SAFETY: the old payload has been released and the slot remains writable.
                        bailed(unsafe { sys::swr_zend_try_facade_array_init(property) });
                    }
                }
            }
        } else if property == undefined {
            // SAFETY: preserve the original null-materialization order before rereading.
            bailed(unsafe { sys::swr_zend_try_facade_null(scope, object, name, length) });
            if !bailout::pending() {
                // SAFETY: Zend now owns an initialized declared property slot.
                property = unsafe { read(scope, object, name, length, silent, &raw mut temporary) };
            }
        }
    }
    bailout::finish();
    property
}

/// # Safety
/// Live internal scope/object and name, with a declared property owned by the object.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_property_array(
    scope: *mut zend_class_entry,
    value: *mut zval,
    name: *const c_char,
    length: usize,
) -> *mut zval {
    // SAFETY: the source holds a live object.
    let object = unsafe { swoole_rs_zend::value::obj_of(value) };
    // SAFETY: materialize and read the declared array property.
    let result = unsafe { array_property(scope, object, name, length) };
    bailout::finish();
    result
}

/// # Safety
/// Live internal object and scope; name identifies declared property metadata.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_property_offset(
    scope: *mut zend_class_entry,
    value: *mut zval,
    name: *mut zend_string,
) -> *mut zval {
    // SAFETY: borrow the registered class's property metadata table without an enduring reference.
    let table = unsafe { &raw const (*scope).properties_info };
    // SAFETY: the declared property exists in this table.
    let entry = unsafe { sys::zend_hash_find(table, name) };
    // SAFETY: property metadata is a pointer-valued initialized zval.
    let payload = unsafe { (*entry).value };
    // SAFETY: Zend's properties_info table stores zend_property_info pointers.
    let info = unsafe { payload.ptr }.cast::<sys::zend_property_info>();
    // SAFETY: the object matches the registered scope.
    let object = unsafe { swoole_rs_zend::value::obj_of(value) };
    // SAFETY: a declared property's offset is inside the object's allocation.
    let offset = unsafe { (*info).offset } as usize;
    let property = object.wrapping_byte_add(offset).cast::<zval>();
    // SAFETY: compare Zend's distinguished uninitialized-value address, as the original does.
    if property == unsafe { sys::swr_zend_uninitialized_zval() } {
        // SAFETY: the writable property slot is initialized to null.
        unsafe {
            (*property).u1 = sys::_zval_struct__bindgen_ty_1 {
                type_info: sys::IS_NULL,
            }
        };
    }
    property
}

/// # Safety
/// Live internal object/scope and property name; returned storage belongs to the object.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_property_optional(
    scope: *mut zend_class_entry,
    value: *mut zval,
    name: *mut zend_string,
    silent: bool,
) -> *mut zval {
    // SAFETY: the caller supplies an initialized string.
    let name_length = unsafe { (*name).len };
    // SAFETY: a registered object's zval.
    let object = unsafe { swoole_rs_zend::value::obj_of(value) };
    // SAFETY: zero is an initialized undefined scratch slot.
    let mut temporary: zval = unsafe { std::mem::zeroed() };
    // SAFETY: caller guarantees the result belongs to the object's property storage.
    let result = unsafe {
        read(
            scope,
            object,
            string::val(name).cast(),
            name_length,
            silent,
            &raw mut temporary,
        )
    };
    bailout::finish();
    // SAFETY: a successful property read returned a live zval.
    if matches!(unsafe { type_of(result) }, sys::IS_NULL | sys::IS_UNDEF) {
        ptr::null_mut()
    } else {
        result
    }
}

/// # Safety
/// A live callable frame; copying its arguments takes independent references.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_params_keep(frame: *mut zend_fcall_info) {
    // SAFETY: the frame's argument count is initialized.
    let count = unsafe { (*frame).param_count } as usize;
    if count == 0 {
        return;
    }
    // SAFETY: Zend allocation, initialized to the original ecalloc representation.
    let params = unsafe { super::owned::alloc_request(count * size_of::<zval>()) }.cast::<zval>();
    bailout::finish();
    // SAFETY: fresh allocation holding count zvals.
    unsafe { params.write_bytes(0, count) };
    for index in 0..count {
        // SAFETY: the original frame holds count live argument slots.
        let source = unsafe { (*frame).params }.wrapping_add(index);
        // SAFETY: each source slot is initialized.
        let value = unsafe { source.read() };
        let dest = params.wrapping_add(index);
        // SAFETY: each destination is an independent slot of the new allocation.
        unsafe { dest.write(value) };
        // SAFETY: retain the payload reference now owned by this argument slot.
        unsafe { sys::swr_zval_try_addref(dest) };
    }
    // SAFETY: publish only after all argument references have been retained.
    unsafe { (*frame).params = params };
}

/// # Safety
/// The frame owns its persisted request argument array and its payload references.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_params_free(frame: *mut zend_fcall_info) {
    let mut index = 0;
    // SAFETY: reread the live count after each destructor, matching the original loop.
    if unsafe { (*frame).param_count } != 0 {
        // SAFETY: each iteration checks the current argument bound.
        while index < unsafe { (*frame).param_count } as usize {
            // SAFETY: the frame owns this live argument slot.
            let value = unsafe { (*frame).params }.wrapping_add(index);
            // SAFETY: release the argument inside the C trap before advancing.
            unsafe { destroy(value) };
            if bailout::pending() {
                break;
            }
            index += 1;
        }
        if !bailout::pending() {
            // SAFETY: the live frame still owns its persisted request allocation.
            let params = unsafe { (*frame).params };
            // SAFETY: all argument payloads have been released.
            unsafe { sys::swr_zend_efree(params.cast()) };
        }
    }
    bailout::finish();
}

unsafe fn closure(cache: *const zend_fcall_info_cache) -> *mut zend_object {
    // SAFETY: a resolved cache holds a live function handler.
    let handler = unsafe { (*cache).function_handler };
    // SAFETY: every Zend function initializes its common flags.
    let function = unsafe { handler.read() };
    // SAFETY: every Zend function has an initialized common prefix.
    let common = unsafe { function.common };
    if common.fn_flags & sys::ZEND_ACC_CLOSURE == 0 {
        ptr::null_mut()
    } else {
        // SAFETY: the closure flag makes Zend's version-dependent closure object projection valid.
        unsafe { sys::swr_zend_facade_closure(handler) }
    }
}

/// # Safety
/// A resolved callable cache; the owner accounts for the independent object/closure references.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_cache_keep(cache: *mut zend_fcall_info_cache) {
    // SAFETY: the cache is initialized.
    let object = unsafe { (*cache).object };
    if !object.is_null() {
        // SAFETY: take a reference to the bound live object.
        unsafe { sys::swr_zend_object_addref(object) };
    }
    // SAFETY: a resolved cache; closure projection does not execute PHP.
    let closure = unsafe { closure(cache) };
    if !closure.is_null() {
        // SAFETY: take a separate closure reference, even if it is also the bound object.
        unsafe { sys::swr_zend_object_addref(closure) };
    }
}

/// # Safety
/// A resolved cache owns the references added by `cache_keep`; each release may execute PHP.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_cache_free(cache: *mut zend_fcall_info_cache) {
    // SAFETY: read the initialized cache without keeping a Rust borrow across PHP.
    let object = unsafe { (*cache).object };
    if !object.is_null() {
        // SAFETY: release the bound object's reference inside Zend's C trap.
        bailout::defer(unsafe {
            bailout::trap(sys::swr_zend_facade_object_release, object.cast())
        });
    }
    if !bailout::pending() {
        // SAFETY: reread the function after releasing the object, as the original does.
        let closure = unsafe { closure(cache) };
        if !closure.is_null() {
            // SAFETY: release the separately held closure reference inside the C trap.
            bailout::defer(unsafe {
                bailout::trap(sys::swr_zend_facade_object_release, closure.cast())
            });
        }
    }
    bailout::finish();
}

unsafe fn fatal_exception() {
    // SAFETY: observe the executor exception only after the preceding call returned.
    let exception = unsafe { sys::swr_zend_facade_exception() };
    if !exception.is_null() {
        // SAFETY: Zend's fatal exception reporter runs inside the C trap.
        bailout::defer(unsafe {
            bailout::trap(sys::swr_zend_facade_exception_error, exception.cast())
        });
    }
}

unsafe fn direct_call(
    name: *mut zval,
    cache: *mut zend_fcall_info_cache,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
    report: unsafe extern "C" fn(*const c_char),
) -> c_int {
    // SAFETY: zero is an undefined optional function name and empty fcall info.
    let mut call: zend_fcall_info = unsafe { std::mem::zeroed() };
    // SAFETY: the temporary return slot starts undefined.
    let mut temporary: zval = unsafe { std::mem::zeroed() };
    // SAFETY: a non-null cache is initialized by the caller.
    let uncached = cache.is_null() || unsafe { (*cache).function_handler }.is_null();
    if uncached {
        if name.is_null() {
            // SAFETY: the native reporter executes entirely within Zend's C trap.
            bailout::defer(unsafe { bailout::report(report, c"Bad function") });
            return -1;
        }
        // SAFETY: a live function-name zval, borrowed through invocation.
        call.function_name = unsafe { name.read() };
    }
    call.size = size_of::<zend_fcall_info>();
    call.retval = if retval.is_null() {
        &raw mut temporary
    } else {
        retval
    };
    call.param_count = argc;
    call.params = argv;
    // SAFETY: initialized frame/cache/arguments; no Rust borrow spans invocation.
    let result = unsafe { bailout::call(&raw mut call, cache) };
    let result = match result {
        Ok(true) => 0,
        Ok(false) => -1,
        Err(error) => {
            error.defer();
            -1
        }
    };
    if retval.is_null() && !bailout::pending() {
        // SAFETY: discard the private return slot after invocation, as upstream does.
        unsafe { destroy(&raw mut temporary) };
    }
    result
}

/// # Safety
/// Live function-name/cache/argument zvals and optional return storage, as Zend requires.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_call(
    name: *mut zval,
    cache: *mut zend_fcall_info_cache,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
    fatal: bool,
    report: unsafe extern "C" fn(*const c_char),
) -> c_int {
    // SAFETY: forwarding the caller's initialized arguments and native warning reporter.
    let result = unsafe { direct_call(name, cache, argc, argv, retval, report) };
    if fatal && !bailout::pending() {
        // SAFETY: preserve the immediate fatal exception check for loop callbacks.
        unsafe { fatal_exception() };
    }
    bailout::finish();
    result
}

/// # Safety
/// Live callable frame/cache; caller accepts the frame's upstream temporary-return convention.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_call_anyway(
    frame: *mut zend_fcall_info,
    cache: *mut zend_fcall_info_cache,
) -> c_int {
    // SAFETY: zero is an initialized undefined return slot.
    let mut temporary: zval = unsafe { std::mem::zeroed() };
    // SAFETY: the frame is initialized and exclusively configured before invocation.
    if unsafe { (*frame).retval }.is_null() {
        // SAFETY: the temporary remains live through the call and destruction below.
        unsafe { (*frame).retval = &raw mut temporary };
    }
    // SAFETY: Zend executes the configured frame entirely inside the C trap.
    let result = unsafe { bailout::call(frame, cache) };
    let result = match result {
        Ok(true) => 0,
        Ok(false) => -1,
        Err(error) => {
            error.defer();
            -1
        }
    };
    // SAFETY: reread the frame after invocation, matching the original helper.
    if !bailout::pending() && unsafe { (*frame).retval } == &raw mut temporary {
        // SAFETY: the private temporary owns any returned payload.
        unsafe { destroy(&raw mut temporary) };
    }
    bailout::finish();
    result
}

/// # Safety
/// A live resolved cache, argument span and optional return slot. `create` is the C++ core leaf.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_function_call(
    cache: *mut zend_fcall_info_cache,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
    coroutine: bool,
    create: unsafe extern "C" fn(*mut zend_fcall_info_cache, u32, *mut zval) -> i64,
    report: unsafe extern "C" fn(*const c_char),
) -> bool {
    let success = if coroutine {
        if !retval.is_null() {
            // SAFETY: coroutines publish null before creation and have no return value.
            unsafe {
                (*retval).u1 = sys::_zval_struct__bindgen_ty_1 {
                    type_info: sys::IS_NULL,
                }
            };
        }
        let mut result = -1;
        // SAFETY: the native coroutine leaf runs inside the C trap; no Rust borrow spans a switch.
        bailed(unsafe {
            sys::swr_zend_try_facade_coroutine(Some(create), cache, argc, argv, &raw mut result)
        });
        result >= 0
    } else {
        // SAFETY: the caller lends a resolved cache and live arguments.
        unsafe { direct_call(ptr::null_mut(), cache, argc, argv, retval, report) == 0 }
    };
    if !bailout::pending() {
        // SAFETY: callbacks outside the VM must report an exception immediately.
        unsafe { fatal_exception() };
    }
    bailout::finish();
    success
}

/// # Safety
/// A borrowed binary function-name span, argument slots and writable owned return value.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_named_call(
    name: *const c_char,
    length: usize,
    argc: u32,
    argv: *mut zval,
    retval: *mut zval,
) {
    // SAFETY: zero initializes the function-name slot before its string allocation.
    let mut function: zval = unsafe { std::mem::zeroed() };
    // SAFETY: Zend copies the complete binary function name into its request string.
    bailed(unsafe { sys::swr_zend_try_facade_value_string(&raw mut function, name, length) });
    bailout::finish();
    let mut result = -1;
    // SAFETY: Zend's macro invocation is one leaf effect inside the C trap.
    bailed(unsafe {
        sys::swr_zend_try_facade_user_call(&raw mut function, argc, argv, retval, &raw mut result)
    });
    if !bailout::pending() {
        if result != 0 {
            // SAFETY: failed named invocation publishes null before name cleanup.
            unsafe {
                (*retval).u1 = sys::_zval_struct__bindgen_ty_1 {
                    type_info: sys::IS_NULL,
                }
            };
        }
        // SAFETY: the local function-name string owns one reference.
        unsafe { destroy(&raw mut function) };
        if !bailout::pending() {
            // SAFETY: preserve the fatal exception check after function-name cleanup.
            unsafe { fatal_exception() };
        }
    }
    bailout::finish();
}
