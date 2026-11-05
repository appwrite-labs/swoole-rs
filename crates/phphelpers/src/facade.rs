//! Copied-scalar `KeyValue` and iterator policy; Zend storage and effects stay native.
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
}

/// # Safety
/// Initialized independent phase0. Operations0/1/2 are `KeyValue` construction,
/// insertion/destruction. Native refreshes key presence on `ObserveKey` and owns
/// every value and effect.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_value_plan(
    phase: *mut u32,
    operation: u32,
    key_present: u8,
) -> u32 {
    use ValueAction::{
        Copy, Dereference, Destroy, Done, InsertIndex, InsertString, NullKey, ObserveKey,
        ReleaseKey, Retain, RetainInserted, RetainKey,
    };
    // SAFETY: `phase` is null or the caller's aligned, exclusively owned phase storage.
    let Some(phase) = (unsafe { phase.as_mut() }) else {
        return Done as u32;
    };
    let (action, next) = match (operation, *phase) {
        (0..=2, 0) => (ObserveKey, 1),
        (0, 1) => (if key_present != 0 { RetainKey } else { NullKey }, 2),
        (0, 2) => (Dereference, 3),
        (0, 3) => (Copy, 4),
        (0, 4) => (Retain, 5),
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
        _ => (Done, 6),
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
