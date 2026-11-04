//! Copied-scalar lifecycle plans for the public native Zend facade.
//! All storage, conversion, reference-counting and destructor effects stay native.
#[repr(u32)]
enum Action {
    Done = 0,
    Observe = 1,
    Release = 2,
    Reset = 3,
    Copy = 4,
    Move = 5,
    ResetSource = 6,
    Alias = 7,
}

/// # Safety
/// Phase is initialized independent native storage. Observation is refreshed only
/// for Observe (presence/self identity) or the constructor's copied retain flag.
/// Native executes each returned effect after Rust returns; no Zend pointer enters.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_string_plan(
    phase: *mut u32,
    operation: u32,
    observation: u8,
) -> u32 {
    use Action::{Alias, Copy, Done, Move, Observe, Release, Reset, ResetSource};
    // SAFETY: `phase` is null or the caller's aligned, exclusively owned phase storage.
    let Some(phase) = (unsafe { phase.as_mut() }) else {
        return Done as u32;
    };
    let (action, next) = match (operation, *phase) {
        // Shared String/CharPtr release, including publication after destruction.
        (0, 0) => (Observe, 1),
        (0, 1) if observation != 0 => (Release, 2),
        (0, 2) => (Reset, 3),
        // String copy assignment: self guard precedes destruction/conversion.
        (1, 0) => (Observe, 1),
        (1, 1) if observation == 0 => (Release, 2),
        (1, 2) => (Copy, 3),
        // String move assignment preserves the original release-before-transfer,
        // even for self-move: do not introduce a new self-assignment behavior.
        (2 | 3, 0) => (Release, 1),
        (2, 1) => (Move, 2),
        (2, 2) => (ResetSource, 3),
        // String(zval*) assignment and both CharPtr duplication assignments.
        (3, 1) => (Copy, 2),
        // Borrowed native zend_string constructor: retain or take custody.
        (4, 0) => (if observation != 0 { Copy } else { Alias }, 1),
        _ => (Done, 3),
    };
    *phase = next;
    action as u32
}

#[repr(u32)]
enum ValueAction {
    Done = 0,
    ObserveKey = 1,
    RetainKey = 2,
    NullKey = 3,
    Dereference = 4,
    Copy = 5,
    Retain = 6,
    InsertString = 7,
    InsertIndex = 8,
    RetainInserted = 9,
    ReleaseKey = 10,
    Destroy = 11,
    ResetSource = 12,
}

/// # Safety
/// Initialized independent phase0. Operations0/1/2 are `KeyValue` construction,
/// insertion/destruction;3/4 are Variable retained copy/move. Key presence is
/// refreshed by native `ObserveKey` before use. Native owns every value and effect.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_value_plan(
    phase: *mut u32,
    operation: u32,
    key_present: u8,
) -> u32 {
    use ValueAction::{
        Copy, Dereference, Destroy, Done, InsertIndex, InsertString, NullKey, ObserveKey,
        ReleaseKey, ResetSource, Retain, RetainInserted, RetainKey,
    };
    // SAFETY: `phase` is null or the caller's aligned, exclusively owned phase storage.
    let Some(phase) = (unsafe { phase.as_mut() }) else {
        return Done as u32;
    };
    let (action, next) = match (operation, *phase) {
        (0..=2, 0) => (ObserveKey, 1),
        (0, 1) => (if key_present != 0 { RetainKey } else { NullKey }, 2),
        (0, 2) => (Dereference, 3),
        (0, 3) | (3 | 4, 0) => (Copy, *phase + 1),
        (0, 4) | (3, 1) => (Retain, *phase + 1),
        (1, 1) => (
            if key_present != 0 {
                InsertString
            } else {
                InsertIndex
            },
            2,
        ),
        (1, 2) => (RetainInserted, 3),
        (2, 1) if key_present != 0 => (ReleaseKey, 2),
        (2, 1 | 2) => (Destroy, 3),
        (4, 1) => (ResetSource, 2),
        _ => (Done, 6),
    };
    *phase = next;
    action as u32
}

/// # Safety
/// Initialized native phase0. Name presence is observed only at action4 after
/// native cache/value copy and retention. No callable or string pointer enters.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_duplicate_plan(
    phase: *mut u32,
    name_present: u8,
) -> u32 {
    // SAFETY: `phase` is null or the caller's aligned, exclusively owned phase storage.
    let Some(phase) = (unsafe { phase.as_mut() }) else {
        return 0;
    };
    let action = match *phase {
        0..=3 => *phase + 1,
        4 if name_present != 0 => 5,
        _ => 0,
    };
    *phase = phase.saturating_add(1);
    action
}

#[repr(u32)]
enum FactoryAction {
    Null = 0,
    ObserveReady = 1,
    Delete = 2,
    Return = 3,
    ObservePointer = 4,
    ObserveNull = 5,
    Create = 6,
    ObserveCreated = 7,
    NullError = 8,
    InvalidError = 9,
}

/// # Safety
/// Initialized native phase0; operation0 basic factory,1 nullable facade. Facts
/// are refreshed only on Observe actions; `allow_null` is the copied native flag.
/// Allocation, constructor warnings/destruction and exceptions remain native.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_factory_plan(
    phase: *mut u32,
    operation: u32,
    observation: u8,
    allow_null: u8,
) -> u32 {
    use FactoryAction::{
        Create, Delete, InvalidError, Null, NullError, ObserveCreated, ObserveNull, ObservePointer,
        ObserveReady, Return,
    };
    // SAFETY: `phase` is null or the caller's aligned, exclusively owned phase storage.
    let Some(phase) = (unsafe { phase.as_mut() }) else {
        return Null as u32;
    };
    let null_result = if allow_null != 0 { Null } else { NullError };
    let (action, next) = match (operation, *phase) {
        (0, 0) => (ObserveReady, 1),
        (0, 1) if observation != 0 => (Return, 9),
        (0, 1) => (Delete, 9),
        (1, 0) => (ObservePointer, 1),
        (1, 1) if observation == 0 => (null_result, 9),
        (1, 1) => (ObserveNull, 2),
        (1, 2) if observation != 0 => (null_result, 9),
        (1, 2) => (Create, 3),
        (1, 3) => (ObserveCreated, 4),
        (1, 4) if observation != 0 => (Return, 9),
        (1, 4) => (InvalidError, 9),
        _ => (Null, 9),
    };
    *phase = next;
    action as u32
}

#[repr(u32)]
enum IteratorAction {
    Done = 0,
    ObserveEnd = 1,
    ReadValue = 2,
    ObservePointer = 3,
    ObserveIndirect = 4,
    Dereference = 5,
    ObserveUndefined = 6,
    Advance = 7,
    ObserveKey = 8,
    PublishString = 9,
    PublishIndex = 10,
}

/// # Safety
/// Initialized phase0; fresh native scalar observation only after Observe actions.
/// Native owns Bucket bounds, zval decoding, cursor/value/key/index storage. No
/// pointers or native layout are passed to Rust; no native work within this call.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_iterator_plan(
    phase: *mut u32,
    observation: u8,
) -> u32 {
    use IteratorAction::{
        Advance, Dereference, Done, ObserveEnd, ObserveIndirect, ObserveKey, ObservePointer,
        ObserveUndefined, PublishIndex, PublishString, ReadValue,
    };
    // SAFETY: `phase` is null or the caller's aligned, exclusively owned phase storage.
    let Some(phase) = (unsafe { phase.as_mut() }) else {
        return Done as u32;
    };
    let (action, next) = match *phase {
        0 | 7 => (ObserveEnd, 1),
        1 if observation != 0 => (Done, 99),
        1 => (ReadValue, 2),
        2 => (ObservePointer, 3),
        3 if observation != 0 => (ObserveIndirect, 4),
        3 => (ObserveUndefined, 6),
        4 if observation != 0 => (Dereference, 5),
        4 | 5 => (ObserveUndefined, 6),
        6 if observation != 0 => (Advance, 7),
        6 => (ObserveKey, 8),
        8 => (
            if observation != 0 {
                PublishString
            } else {
                PublishIndex
            },
            9,
        ),
        _ => (Done, 99),
    };
    *phase = next;
    action as u32
}
