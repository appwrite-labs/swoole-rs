//! PHP HTTP policy, whose native effects execute only after these functions
//! return. The PHP classes over it live in crates/phphttp (docs/zend.md).
#![cfg_attr(
    test,
    expect(
        clippy::disallowed_types,
        reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
    )
)]
#![forbid(unsafe_code)]
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

/// One step of [`swr_v1_httpbindings_settings`].
#[derive(Default)]
pub struct Action {
    pub kind: u32,
    /// The settings key to look up, for a lookup.
    pub text: &'static [u8],
}
