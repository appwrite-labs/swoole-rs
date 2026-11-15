//! Owned DNS request strings, hints and address storage. A record is shared as an
//! `Arc`: its request holds one reference and each pool copy of an `AsyncEvent`
//! carrying it another. Records contain no callbacks.
use crate::resolver::{DIAGNOSTIC_FAILED, gethostbyname, ntop, resolve_addresses};
use core::ffi::{c_char, c_int};
use std::ffi::CString;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ResolverFields {
    pub family: c_int,
    pub socktype: c_int,
    pub protocol: c_int,
    pub error: c_int,
    pub count: c_int,
}
/// Why a [`ResolverRequest`] lookup failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// The name did not resolve (libc's error is in the fields).
    Failed,
    /// An allocation was refused.
    Alloc,
    /// The unknown-family hook refused the lookup.
    Unknown,
}

/// The request a pool thread and its owner share: each field behind its own lock,
/// held for one read or write.
pub struct ResolverRequest {
    fields: Mutex<ResolverFields>,
    name: Vec<u8>,
    service: Vec<u8>,
    /// The text address `host` wrote.
    address: Mutex<Vec<u8>>,
    results: Mutex<Vec<libc::sockaddr_in6>>,
}

fn zero_address() -> libc::sockaddr_in6 {
    crate::sys::zeroed()
}

fn locked<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A copy of `items`, or none on allocation refusal.
fn owned<T: Copy>(items: &[T]) -> Option<Vec<T>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(items.len()).ok()?;
    copy.extend_from_slice(items);
    Some(copy)
}

/// `bytes` as `std::string::c_str()` reads them: up to the first NUL.
fn c_string(mut bytes: Vec<u8>) -> CString {
    if let Some(end) = bytes.iter().position(|&byte| byte == 0) {
        bytes.truncate(end);
    }
    CString::new(bytes).unwrap_or_default()
}

/// `text` up to its terminator.
fn terminated(text: &[c_char; 46]) -> Vec<u8> {
    let end = text
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(text.len());
    text.iter()
        .take(end)
        .map(|&byte| u8::from_ne_bytes(byte.to_ne_bytes()))
        .collect()
}

impl ResolverRequest {
    /// A record for `name` and `service`; none on allocation refusal.
    #[must_use]
    pub fn create(
        family: c_int,
        socktype: c_int,
        protocol: c_int,
        name: &[u8],
        service: &[u8],
    ) -> Option<Arc<Self>> {
        Some(Arc::new(Self {
            fields: Mutex::new(ResolverFields {
                family,
                socktype,
                protocol,
                error: 0,
                count: 0,
            }),
            name: owned(name)?,
            service: owned(service)?,
            address: Mutex::new(Vec::new()),
            results: Mutex::new(Vec::new()),
        }))
    }

    /// The hints and results.
    #[must_use]
    pub fn fields(&self) -> ResolverFields {
        *locked(&self.fields)
    }

    /// Changes the hints and results.
    pub fn update(&self, change: impl FnOnce(&mut ResolverFields)) {
        change(&mut locked(&self.fields));
    }

    /// The name to resolve.
    #[must_use]
    pub fn name(&self) -> Vec<u8> {
        self.name.clone()
    }

    /// The text address `host` wrote.
    #[must_use]
    pub fn address(&self) -> Vec<u8> {
        locked(&self.address).clone()
    }

    /// `async::handler_getaddrinfo`'s body: resolves the hints and replaces the
    /// complete result set. A libc error publishes only `error`, keeping the
    /// previous count and addresses. On success count and error publish before
    /// allocation, as native vector assignment does.
    /// # Errors
    /// [`ResolveError::Failed`] for a libc error, [`ResolveError::Alloc`] for an
    /// allocation refusal, [`ResolveError::Unknown`] when `unknown` returned false.
    pub fn lookup(&self, unknown: impl FnMut(c_int) -> bool) -> Result<(), ResolveError> {
        let hints = self.fields();
        let name = c_string(self.name.clone());
        let service = (!self.service.is_empty()).then(|| c_string(self.service.clone()));
        let mut found = [zero_address(); 128];
        let result = resolve_addresses(
            &name,
            service.as_deref(),
            (hints.family, hints.socktype, hints.protocol),
            &mut found,
            unknown,
        );
        if result.error == DIAGNOSTIC_FAILED {
            return Err(ResolveError::Unknown);
        }
        self.update(|fields| fields.error = result.error);
        if result.error != 0 {
            return Err(ResolveError::Failed);
        }
        self.update(|fields| fields.count = result.count);
        let count = usize::try_from(result.count).unwrap_or(0);
        let Some(found) = owned(found.get(..count).unwrap_or_default()) else {
            return Err(ResolveError::Alloc);
        };
        *locked(&self.results) = found;
        Ok(())
    }

    /// `async::handler_gethostbyname`'s body: the name's first address, as text,
    /// into the address slot.
    /// # Errors
    /// [`ResolveError::Failed`] when it does not resolve.
    pub fn host(&self) -> Result<(), ResolveError> {
        let family = self.fields().family;
        let text = gethostbyname(family, &c_string(self.name()))
            .and_then(|binary| ntop(family, binary.first_chunk()?));
        let Some(text) = text else {
            return Err(ResolveError::Failed);
        };
        *locked(&self.address) = terminated(&text);
        Ok(())
    }

    /// Address `index` as text, or none past the end; `valid` false if it does not
    /// format.
    fn text(&self, index: usize) -> Option<ResolverText> {
        let family = self.fields().family;
        let address = *locked(&self.results).get(index)?;
        let binary = if family == libc::AF_INET6 {
            address.sin6_addr.s6_addr
        } else {
            // The original reads other families through sockaddr_in, whose sin_addr
            // shares sin6_flowinfo's offset.
            let mut binary = [0; 16];
            binary[..4].copy_from_slice(&address.sin6_flowinfo.to_ne_bytes());
            binary
        };
        let binary = binary.map(|byte| c_char::from_ne_bytes(byte.to_ne_bytes()));
        let text = ntop(family, &binary);
        Some(ResolverText {
            bytes: text.unwrap_or([0; 46]),
            valid: text.is_some(),
        })
    }

    /// The text addresses `lookup` found, skipping any that do not format.
    #[must_use]
    pub fn addresses(&self) -> Vec<Vec<u8>> {
        (0..)
            .map_while(|index| self.text(index))
            .filter(|text| text.valid)
            .map(|text| terminated(&text.bytes))
            .collect()
    }

    /// A copy of the socket addresses `lookup` found.
    #[must_use]
    pub fn results(&self) -> Vec<libc::sockaddr_in6> {
        locked(&self.results).clone()
    }
}

/// One address as text; `valid` false if it does not format.
struct ResolverText {
    bytes: [c_char; 46],
    valid: bool,
}

#[repr(C)]
pub struct ResolverCompletion {
    pub error: c_int,
    pub failed: bool,
}
impl ResolverCompletion {
    /// Decides completion after an await returns. A failed await must not read the
    /// worker-owned request error, which may still be changing after timeout, so
    /// `request_error` runs only when `retval != -1`.
    pub fn after_await(
        retval: isize,
        event_error: c_int,
        request_error: impl FnOnce() -> c_int,
        timeout_error: c_int,
        resolution_timeout: c_int,
    ) -> Self {
        Self {
            failed: retval == -1 || request_error() != 0,
            error: if event_error == timeout_error {
                resolution_timeout
            } else {
                event_error
            },
        }
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    #[test]
    fn failed_await_never_reads_worker_error() {
        let inaccessible = || unreachable!("a failed await reads no request error");
        let completion = ResolverCompletion::after_await(-1, 4003, inaccessible, 4003, 712);
        assert!(completion.failed);
        assert_eq!(completion.error, 712);
        let completed_error = 7;
        assert!(ResolverCompletion::after_await(0, 0, || completed_error, 4003, 712).failed);
        assert!(!ResolverCompletion::after_await(1, 0, || 0, 4003, 712).failed);
    }
}
