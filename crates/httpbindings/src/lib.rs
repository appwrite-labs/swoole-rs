//! PHP HTTP policy, whose native effects execute only after these functions
//! return. The PHP classes over it live in crates/phphttp (docs/zend.md).
#![cfg_attr(
    test,
    expect(
        clippy::disallowed_types,
        reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
    )
)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
mod file;
mod payload;
mod request;
mod response;
mod server;
mod settings;
pub use file::*;
pub use payload::*;
pub use request::*;
pub use response::*;
pub use server::*;
pub use settings::*;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Text {
    pub data: *const u8,
    pub length: usize,
}

#[repr(C)]
pub struct CookieInput {
    /// name, value, path, domain, sameSite, priority; null preserves absence.
    pub fields: [Text; 6],
    pub expires: i64,
    /// encode, secure, httpOnly, partitioned.
    pub flags: u32,
    pub age: f64,
}

#[repr(C)]
#[derive(Default)]
pub struct Action {
    pub kind: u32,
    pub text: Text,
    pub value: i64,
}

#[derive(Clone, Copy)]
#[repr(u32)]
enum Phase {
    ValidateName,
    Name,
    ValueChoice,
    ValidateValue,
    Equals,
    Value,
    Encoded,
    FreeEncoded,
    ExpiryChoice,
    ValidateExpiry,
    ExpiryPrefix,
    ExpiryDate,
    Date,
    AgePrefix,
    ReadAge,
    Age,
    FreeDate,
    DeletePrefix,
    DeleteDate,
    DeletedDate,
    DeleteAge,
    FreeDeletedDate,
    ValidatePath,
    PathPrefix,
    Path,
    ValidateDomain,
    DomainPrefix,
    Domain,
    Secure,
    HttpOnly,
    SameSitePrefix,
    SameSite,
    PriorityPrefix,
    Priority,
    Partitioned,
    Done,
}

const PHASES: [Phase; 36] = [
    Phase::ValidateName,
    Phase::Name,
    Phase::ValueChoice,
    Phase::ValidateValue,
    Phase::Equals,
    Phase::Value,
    Phase::Encoded,
    Phase::FreeEncoded,
    Phase::ExpiryChoice,
    Phase::ValidateExpiry,
    Phase::ExpiryPrefix,
    Phase::ExpiryDate,
    Phase::Date,
    Phase::AgePrefix,
    Phase::ReadAge,
    Phase::Age,
    Phase::FreeDate,
    Phase::DeletePrefix,
    Phase::DeleteDate,
    Phase::DeletedDate,
    Phase::DeleteAge,
    Phase::FreeDeletedDate,
    Phase::ValidatePath,
    Phase::PathPrefix,
    Phase::Path,
    Phase::ValidateDomain,
    Phase::DomainPrefix,
    Phase::Domain,
    Phase::Secure,
    Phase::HttpOnly,
    Phase::SameSitePrefix,
    Phase::SameSite,
    Phase::PriorityPrefix,
    Phase::Priority,
    Phase::Partitioned,
    Phase::Done,
];

pub const APPEND: u32 = 1;
pub const WARN: u32 = 2;
pub const ENCODE: u32 = 3;
pub const APPEND_ENCODED: u32 = 4;
pub const FREE_ENCODED: u32 = 5;
pub const FORMAT_DATE: u32 = 6;
pub const APPEND_DATE: u32 = 7;
pub const FREE_DATE: u32 = 8;
pub const READ_AGE: u32 = 9;
pub const APPEND_AGE: u32 = 10;
pub const DONE: u32 = 11;
const ILLEGAL: &[u8] = b",; \t\r\n\x0b\x0c";

fn literal(kind: u32, bytes: &'static [u8]) -> Action {
    Action {
        kind,
        text: Text {
            data: bytes.as_ptr(),
            length: bytes.len(),
        },
        value: 0,
    }
}
fn effect(kind: u32) -> Action {
    Action {
        kind,
        ..Action::default()
    }
}
fn append(text: Text) -> Action {
    Action {
        kind: APPEND,
        text,
        value: 0,
    }
}
fn illegal(bytes: &[u8], name: bool) -> bool {
    // The original strpbrk stops at the first NUL, even inside zend_string.
    bytes
        .iter()
        .take_while(|&&c| c != 0)
        .any(|c| ILLEGAL.contains(c) || (name && *c == b'='))
}

/// Select one native effect, observing current fields anew after every effect.
///
/// # Safety
/// Input/output/phase must be initialized, valid, disjoint objects; each nonempty
/// field spans initialized readable bytes for this call. No pointer is retained.
#[expect(
    clippy::too_many_lines,
    reason = "a decision table, one arm per state; it goes when the operation moves into Rust whole"
)]
pub unsafe fn swr_v1_httpbindings_cookie(
    input: *const CookieInput,
    phase: *mut u32,
    output: *mut Action,
) -> i32 {
    // SAFETY: `input` is null or initialized, aligned and lent for this call.
    let input = unsafe { input.as_ref() };
    // SAFETY: `phase` is null or aligned, writable and disjoint from the other objects.
    let phase = unsafe { phase.as_mut() };
    // SAFETY: `output` is null or aligned, writable and disjoint from the other objects.
    let output = unsafe { output.as_mut() };
    let (Some(input), Some(phase), Some(output)) = (input, phase, output) else {
        return -1;
    };
    *output = Action::default();
    let mut fields: [&[u8]; 6] = [&[]; 6];
    for (field, text) in fields.iter_mut().zip(&input.fields) {
        if text.length > isize::MAX as usize || (text.data.is_null() && text.length != 0) {
            return -1;
        }
        if text.length != 0 {
            // SAFETY: non-null, `length <= isize::MAX`, and each nonempty field spans that
            // many initialized bytes for this call.
            *field = unsafe { core::slice::from_raw_parts(text.data, text.length) };
        }
    }
    loop {
        let Some(&current) = PHASES.get(*phase as usize) else {
            return -1;
        };
        *phase += 1;
        let action = match current {
            Phase::ValidateName => {
                if fields[0].is_empty() {
                    literal(WARN, b"The name cannot be empty\0")
                } else if illegal(fields[0], true) {
                    literal(WARN, b"The name cannot contain \"=\", \",\", \";\", \" \", \"\\t\", \"\\r\", \"\\n\", \"\\013\", or \"\\014\"\0")
                } else {
                    continue;
                }
            }
            Phase::Name => append(input.fields[0]),
            Phase::ValueChoice => {
                *phase = if input.fields[1].data.is_null() {
                    Phase::DeletePrefix as u32
                } else {
                    Phase::ValidateValue as u32
                };
                continue;
            }
            Phase::ValidateValue => {
                if input.flags & 1 == 0 && illegal(fields[1], false) {
                    literal(WARN, b"The value cannot contain \",\", \";\", \" \", \"\\t\", \"\\r\", \"\\n\", \"\\013\", or \"\\014\"\0")
                } else {
                    continue;
                }
            }
            Phase::Equals => literal(APPEND, b"="),
            Phase::Value => {
                if input.flags & 1 != 0 {
                    effect(ENCODE)
                } else {
                    *phase = Phase::ExpiryChoice as u32;
                    append(input.fields[1])
                }
            }
            Phase::Encoded => effect(APPEND_ENCODED),
            Phase::FreeEncoded => effect(FREE_ENCODED),
            Phase::ExpiryChoice => {
                if input.expires <= 0 {
                    *phase = Phase::ValidatePath as u32;
                }
                continue;
            }
            Phase::ValidateExpiry => {
                if input.expires >= 253_402_300_800 {
                    literal(WARN, b"The expires cannot have a year greater than 9999\0")
                } else {
                    continue;
                }
            }
            Phase::ExpiryPrefix => literal(APPEND, b"; expires="),
            Phase::ExpiryDate => Action {
                value: input.expires,
                ..effect(FORMAT_DATE)
            },
            Phase::Date | Phase::DeletedDate => effect(APPEND_DATE),
            Phase::AgePrefix => literal(APPEND, b"; Max-Age="),
            Phase::ReadAge => effect(READ_AGE),
            Phase::Age => Action {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "PHP's (zend_long) Max-Age; Rust saturates where C is undefined"
                )]
                value: if input.age >= 0.0 {
                    input.age as i64
                } else {
                    0
                },
                ..effect(APPEND_AGE)
            },
            Phase::FreeDate => {
                *phase = Phase::ValidatePath as u32;
                effect(FREE_DATE)
            }
            Phase::DeletePrefix => literal(APPEND, b"=deleted; expires="),
            Phase::DeleteDate => Action {
                value: 1,
                ..effect(FORMAT_DATE)
            },
            Phase::DeleteAge => literal(APPEND, b"; Max-Age=0"),
            Phase::FreeDeletedDate => effect(FREE_DATE),
            Phase::ValidatePath => {
                if fields[2].is_empty() {
                    *phase = Phase::ValidateDomain as u32;
                    continue;
                }
                if illegal(fields[2], false) {
                    literal(WARN, b"The path option cannot contain \",\", \";\", \" \", \"\\t\", \"\\r\", \"\\n\", \"\\013\", or \"\\014\"\0")
                } else {
                    continue;
                }
            }
            Phase::PathPrefix => literal(APPEND, b"; path="),
            Phase::Path => append(input.fields[2]),
            Phase::ValidateDomain => {
                if fields[3].is_empty() {
                    *phase = Phase::Secure as u32;
                    continue;
                }
                if illegal(fields[3], false) {
                    literal(WARN, b"The domain option cannot contain \",\", \";\", \" \", \"\\t\", \"\\r\", \"\\n\", \"\\013\", or \"\\014\"\0")
                } else {
                    continue;
                }
            }
            Phase::DomainPrefix => literal(APPEND, b"; domain="),
            Phase::Domain => append(input.fields[3]),
            Phase::Secure => {
                if input.flags & 2 != 0 {
                    literal(APPEND, b"; secure")
                } else {
                    continue;
                }
            }
            Phase::HttpOnly => {
                if input.flags & 4 != 0 {
                    literal(APPEND, b"; HttpOnly")
                } else {
                    continue;
                }
            }
            Phase::SameSitePrefix => {
                if fields[4].is_empty() {
                    *phase = Phase::PriorityPrefix as u32;
                    continue;
                }
                literal(APPEND, b"; SameSite=")
            }
            Phase::SameSite => append(input.fields[4]),
            Phase::PriorityPrefix => {
                if fields[5].is_empty() {
                    *phase = Phase::Partitioned as u32;
                    continue;
                }
                literal(APPEND, b"; Priority=")
            }
            Phase::Priority => append(input.fields[5]),
            Phase::Partitioned => {
                if input.flags & 8 != 0 {
                    literal(APPEND, b"; Partitioned")
                } else {
                    continue;
                }
            }
            Phase::Done => effect(DONE),
        };
        *output = action;
        return 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> CookieInput {
        CookieInput {
            fields: [Text::default(); 6],
            expires: 0,
            flags: 0,
            age: 0.0,
        }
    }

    #[test]
    fn rejects_missing_abi_objects() {
        let mut phase = 0;
        let mut action = Action::default();
        let input = input();
        unsafe {
            assert_eq!(
                swr_v1_httpbindings_cookie(core::ptr::null(), &raw mut phase, &raw mut action),
                -1
            );
            assert_eq!(
                swr_v1_httpbindings_cookie(
                    &raw const input,
                    core::ptr::null_mut(),
                    &raw mut action
                ),
                -1
            );
            assert_eq!(
                swr_v1_httpbindings_cookie(&raw const input, &raw mut phase, core::ptr::null_mut()),
                -1
            );
        }
    }

    #[test]
    fn rejects_invalid_spans_before_reading_bytes() {
        let mut input = input();
        let mut phase = 0;
        let mut action = Action::default();
        input.fields[0].length = 1;
        assert_eq!(
            unsafe {
                swr_v1_httpbindings_cookie(&raw const input, &raw mut phase, &raw mut action)
            },
            -1
        );
        input.fields[0] = Text {
            data: b"x".as_ptr(),
            length: isize::MAX as usize + 1,
        };
        assert_eq!(
            unsafe {
                swr_v1_httpbindings_cookie(&raw const input, &raw mut phase, &raw mut action)
            },
            -1
        );
        assert_eq!(phase, 0);
    }

    #[test]
    fn rejects_invalid_cursor_without_overflow_or_effect() {
        let input = input();
        let mut phase = u32::MAX;
        let mut action = Action {
            kind: 123,
            ..Action::default()
        };
        assert_eq!(
            unsafe {
                swr_v1_httpbindings_cookie(&raw const input, &raw mut phase, &raw mut action)
            },
            -1
        );
        assert_eq!(phase, u32::MAX);
        assert_eq!(action.kind, 0);
    }
}
