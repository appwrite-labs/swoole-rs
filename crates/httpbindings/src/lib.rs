//! PHP HTTP policy, whose native effects execute only after these functions
//! return. The PHP classes over it live in crates/phphttp (docs/zend.md).
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    expect(
        clippy::disallowed_types,
        reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
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

#[derive(Default)]
pub struct Action {
    pub kind: u32,
    /// The key a lookup (kind 1) reads.
    pub text: &'static [u8],
}
