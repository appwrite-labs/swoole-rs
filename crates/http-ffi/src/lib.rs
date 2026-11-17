//! The request reader's state ([`HttpRequest`]) and the HTTP proxy records, over
//! crates/http's safe API.
#![forbid(unsafe_code)]
mod owned_proxy;
pub use owned_proxy::*;
mod request;
pub use request::{HttpForm, HttpRequest, request_free, request_new};
