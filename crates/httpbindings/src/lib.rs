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
#[derive(Default)]
pub struct Action {
    pub kind: u32,
    pub text: Text,
    pub value: i64,
}
