//! Upstream's `ArrayItem`: one PHP value copied out of request memory so any
//! thread can read it back. Strings and serialized objects are copied, arrays
//! become shared tables, containers and thread resources are shared by
//! reference, and sockets by a duplicated descriptor.
use crate::{array::Array, barrier, classes, host, queue::Queue, request};
use std::{cmp::Ordering, ffi::c_int, sync::Arc};
use swoole_rs_corebase::CCast;
use swoole_rs_phplocks::{self as phplocks, Resource, Shareable};
use swoole_rs_zend::{
    Call, Owned, Value as ZValue, ZString, bailout, numeric,
    sys::{
        IS_ARRAY, IS_DOUBLE, IS_FALSE, IS_LONG, IS_NULL, IS_OBJECT, IS_RESOURCE, IS_STRING,
        IS_TRUE, IS_UNDEF,
    },
};

// Upstream's item types beyond Zend's (php_swoole_thread.h).
pub(crate) const IS_ARRAYLIST: u32 = 80;
pub(crate) const IS_QUEUE: u32 = 81;
pub(crate) const IS_LOCK: u32 = 82;
pub(crate) const IS_MAP: u32 = 83;
pub(crate) const IS_BARRIER: u32 = 84;
pub(crate) const IS_ATOMIC: u32 = 85;
pub(crate) const IS_ATOMIC_LONG: u32 = 86;
pub(crate) const IS_PHP_SOCKET: u32 = 96;
pub(crate) const IS_CO_SOCKET: u32 = 97;
pub(crate) const IS_STREAM_SOCKET: u32 = 98;
pub(crate) const IS_SERIALIZED_OBJECT: u32 = 99;

/// What an item holds besides its type.
#[derive(Debug)]
pub(crate) enum Value {
    /// No payload (null, true, false, undef, or the object a failed socket
    /// conversion leaves): upstream's zeroed union, which `incr` still adds to.
    Bare(i64),
    Long(i64),
    Double(f64),
    Str(Box<[u8]>),
    /// An `ArrayList` (`IS_ARRAYLIST`) or Map (`IS_MAP`).
    Array(Arc<Array>),
    Queue(Arc<Queue>),
    /// A Barrier; `None` when stored unconstructed.
    Barrier(Option<Arc<std::sync::Barrier>>),
    /// A Lock, Atomic or Atomic\Long's resource; `None` when stored
    /// unconstructed.
    Resource(Option<Arc<Resource>>),
    /// A duplicated descriptor and, for coroutine sockets, its socket type.
    Socket {
        fd: c_int,
        kind: c_int,
    },
    Serialized(Box<[u8]>),
}

#[derive(Debug)]
pub(crate) struct Item {
    pub(crate) kind: u32,
    pub(crate) value: Value,
}

impl Drop for Item {
    /// `ArrayItem::release`.
    fn drop(&mut self) {
        if let Value::Socket { fd, .. } = self.value {
            // The item owns the duplicated descriptor.
            swoole_rs_corebase::sys::close(fd);
        }
    }
}

fn bare(kind: u32) -> Item {
    Item {
        kind,
        value: Value::Bare(0),
    }
}

impl Item {
    pub(crate) fn long(value: i64) -> Self {
        Self {
            kind: IS_LONG,
            value: Value::Long(value),
        }
    }

    pub(crate) fn double(value: f64) -> Self {
        Self {
            kind: IS_DOUBLE,
            value: Value::Double(value),
        }
    }

    /// `new ArrayItem(zvalue)` (`ArrayItem::store`), in the value's request.
    pub(crate) fn store(value: &ZValue) -> Self {
        let kind = value.kind();
        match kind {
            IS_LONG => Self::long(value.long()),
            IS_DOUBLE => Self::double(value.double()),
            IS_STRING => Self {
                kind,
                value: Value::Str(value.bytes().unwrap_or_default().into()),
            },
            IS_TRUE | IS_FALSE | IS_NULL => bare(kind),
            IS_RESOURCE => {
                let fd = request::stream_cast(value);
                if fd == -1 {
                    socket_error(c"failed to convert to socket fd");
                }
                Self {
                    kind: IS_STREAM_SOCKET,
                    value: Value::Socket { fd, kind: 0 },
                }
            }
            IS_ARRAY => match value.array() {
                Some(table) => Self {
                    kind: if table.is_list() {
                        IS_ARRAYLIST
                    } else {
                        IS_MAP
                    },
                    value: Value::Array(Array::from(table)),
                },
                None => bare(IS_UNDEF),
            },
            IS_OBJECT => Self::store_object(value),
            _ => Self::serialize(value),
        }
    }

    /// The `IS_OBJECT` branch: sockets, containers and thread resources by
    /// reference, anything else serialized.
    fn store_object(object: &ZValue) -> Self {
        if let Some((fd, kind)) = host::co_socket_cast(object) {
            if fd == -1 {
                socket_error(c"failed to convert to socket fd");
            }
            return Self {
                kind: IS_CO_SOCKET,
                value: Value::Socket { fd, kind },
            };
        }
        match host::php_socket_cast(object) {
            -3 => {}
            // upstream breaks out before setting the type.
            -2 => return bare(IS_OBJECT),
            fd => {
                return Self {
                    kind: IS_PHP_SOCKET,
                    value: Value::Socket { fd, kind: 0 },
                };
            }
        }
        if let Some((class, resource)) = phplocks::shared(object) {
            let kind = match class {
                Shareable::Lock => IS_LOCK,
                Shareable::Atomic => IS_ATOMIC,
                Shareable::Long => IS_ATOMIC_LONG,
            };
            return Self {
                kind,
                value: Value::Resource(resource),
            };
        }
        if let Some(shared) = classes::shared(object).or_else(|| barrier::shared(object)) {
            return shared;
        }
        Self::serialize(object)
    }

    /// The default branch: `php_swoole_serialize`, or undef if it failed.
    fn serialize(value: &ZValue) -> Self {
        // A bailout of `__serialize` is deferred.
        let Some(data) = host::serialize(value) else {
            return bare(IS_UNDEF);
        };
        Self {
            kind: IS_SERIALIZED_OBJECT,
            value: Value::Serialized(data),
        }
    }

    /// `ArrayItem::fetch`: the value, or `None` where upstream leaves the slot
    /// untouched (null, booleans aside, and undef). Nothing runs once a bailout
    /// is pending: upstream never ran past it.
    pub(crate) fn fetch(&self) -> Option<Owned> {
        if bailout::pending() {
            return None;
        }
        Some(match &self.value {
            Value::Long(value) => Owned::long(*value),
            Value::Double(value) => Owned::double(*value),
            Value::Bare(_) if self.kind == IS_TRUE || self.kind == IS_FALSE => {
                Owned::bool(self.kind == IS_TRUE)
            }
            Value::Bare(_) => return None,
            Value::Str(text) => Owned::string(ZString::new(text)),
            Value::Array(array) => classes::array_object(self.kind, array),
            Value::Queue(queue) => classes::queue_object(queue),
            Value::Barrier(shared) => barrier::object(shared.as_ref()),
            Value::Resource(resource) => {
                let class = match self.kind {
                    IS_LOCK => Shareable::Lock,
                    IS_ATOMIC => Shareable::Atomic,
                    _ => Shareable::Long,
                };
                phplocks::shared_object(class, resource.as_ref())
            }
            Value::Socket { fd, kind } => match self.kind {
                IS_STREAM_SOCKET => request::stream_create(*fd),
                IS_CO_SOCKET => {
                    request::socket_create(*fd, |fd| host::co_socket_create(fd, *kind))?
                }
                _ => request::socket_create(*fd, host::php_socket_create)?,
            },
            Value::Serialized(data) => host::unserialize(data),
        })
    }

    /// `fetch`, or null where upstream would leave the slot as it was.
    pub(crate) fn fetch_or_null(&self) -> Owned {
        self.fetch().unwrap_or_else(Owned::null)
    }

    /// `ArrayItem::equals`.
    pub(crate) fn equals(&self, search: &ZValue) -> bool {
        if search.kind() != self.kind {
            return false;
        }
        match &self.value {
            Value::Long(value) => search.as_long() == Some(*value),
            Value::Double(value) => search.as_double() == Some(*value),
            Value::Bare(_) => matches!(self.kind, IS_TRUE | IS_FALSE | IS_NULL),
            Value::Str(text) => search.bytes() == Some(&**text),
            _ => false,
        }
    }

    /// The union's bits as upstream reads them through `ITEM_LVAL`/`ITEM_DVAL`
    /// when two types' `TYPE_PAIR`s collide.
    fn raw(&self) -> u64 {
        match &self.value {
            #[expect(clippy::cast_sign_loss, reason = "the union's bits")]
            Value::Bare(value) | Value::Long(value) => *value as u64,
            Value::Double(value) => value.to_bits(),
            #[expect(clippy::cast_sign_loss, reason = "the union's bits")]
            Value::Socket { fd, kind } => (u64::from(*kind as u32) << 32) | u64::from(*fd as u32),
            Value::Resource(resource) => resource
                .as_ref()
                .map_or(0, |r| Arc::as_ptr(r).addr() as u64),
            Value::Array(array) => Arc::as_ptr(array).addr() as u64,
            Value::Queue(queue) => Arc::as_ptr(queue).addr() as u64,
            Value::Barrier(shared) => shared.as_ref().map_or(0, |b| Arc::as_ptr(b).addr() as u64),
            Value::Str(text) | Value::Serialized(text) => text.as_ptr().addr() as u64,
        }
    }

    #[expect(clippy::cast_possible_wrap, reason = "the union's bits")]
    fn lval(&self) -> i64 {
        self.raw() as i64
    }

    fn dval(&self) -> f64 {
        f64::from_bits(self.raw())
    }

    /// `ITEM_STR`: a string's bytes (empty where upstream would read through
    /// another type's union).
    fn text(&self) -> &[u8] {
        match &self.value {
            Value::Str(text) => text,
            _ => &[],
        }
    }

    /// `incr_update`: add to a float or, as upstream's union does, to the int
    /// view of anything else; the type never changes. The sum goes to the
    /// return slot of `call`, a Thread method call.
    pub(crate) fn add(&mut self, delta: &ZValue, call: &Call) {
        match &mut self.value {
            Value::Double(value) => {
                let delta = delta.double();
                if bailout::pending() {
                    return;
                }
                *value += delta;
                call.set_owned(Owned::double(*value));
            }
            Value::Long(value) | Value::Bare(value) => {
                let delta = delta.long();
                if bailout::pending() {
                    return;
                }
                *value = value.wrapping_add(delta);
                call.set_long(*value);
            }
            // Upstream adds to a pointer here; leave the value alone.
            _ => call.set_long(delta.long()),
        }
    }

    /// `incr_create`: a float or int item from `delta`, which is also the
    /// result in the return slot of `call`.
    pub(crate) fn counter(delta: &ZValue, call: &Call) -> Self {
        let item = match delta.as_double() {
            Some(value) => Self::double(value),
            None => Self::long(delta.long()),
        };
        if let Some(value) = item.fetch() {
            call.set_owned(value);
        }
        item
    }

    /// `ArrayItem::compare`, for `zend_hash_sort`. Unsupported pairs throw
    /// upstream's `Error` and sort after.
    pub(crate) fn compare(&self, other: &Self) -> i32 {
        let (a, b) = (self, other);
        let pair = |x: u32, y: u32| (x << 4) | y;
        match (a.kind << 4) | b.kind {
            p if p == pair(IS_LONG, IS_LONG) => sign(a.lval().cmp(&b.lval())),
            p if p == pair(IS_DOUBLE, IS_LONG) => normalize(a.dval() - b.lval().c_cast::<f64>()),
            p if p == pair(IS_LONG, IS_DOUBLE) => normalize(a.lval().c_cast::<f64>() - b.dval()),
            p if p == pair(IS_DOUBLE, IS_DOUBLE) =>
            {
                #[expect(clippy::float_cmp, reason = "upstream compares with ==")]
                if a.dval() == b.dval() {
                    0
                } else {
                    normalize(a.dval() - b.dval())
                }
            }
            p if [
                pair(IS_NULL, IS_NULL),
                pair(IS_NULL, IS_FALSE),
                pair(IS_FALSE, IS_NULL),
                pair(IS_FALSE, IS_FALSE),
                pair(IS_TRUE, IS_TRUE),
            ]
            .contains(&p) =>
            {
                0
            }
            p if p == pair(IS_NULL, IS_TRUE) => -1,
            p if p == pair(IS_TRUE, IS_NULL) => 1,
            p if p == pair(IS_STRING, IS_STRING) => smart_strcmp(a.text(), b.text()),
            p if p == pair(IS_NULL, IS_STRING) => {
                if b.text().is_empty() {
                    0
                } else {
                    -1
                }
            }
            p if p == pair(IS_STRING, IS_NULL) => i32::from(!a.text().is_empty()),
            p if p == pair(IS_LONG, IS_STRING) => long_to_string(a.lval(), b.text()),
            p if p == pair(IS_STRING, IS_LONG) => -long_to_string(b.lval(), a.text()),
            p if p == pair(IS_DOUBLE, IS_STRING) => {
                if a.dval().is_nan() {
                    1
                } else {
                    double_to_string(a.dval(), b.text())
                }
            }
            p if p == pair(IS_STRING, IS_DOUBLE) => {
                if b.dval().is_nan() {
                    1
                } else {
                    -double_to_string(b.dval(), a.text())
                }
            }
            p if p == pair(IS_OBJECT, IS_NULL) => 1,
            p if p == pair(IS_NULL, IS_OBJECT) => -1,
            _ => {
                // A sort Zend runs inside a handler.
                swoole_rs_zend::throw_error(c"Unsupported operand types");
                1
            }
        }
    }
}

/// `zend_throw_exception(swoole_exception_ce, message, errno)`.
fn socket_error(message: &std::ffi::CStr) {
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    swoole_rs_phpswoole::exception().throw(message, i64::from(errno));
}

fn sign(ordering: Ordering) -> i32 {
    match ordering {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// `ZEND_NORMALIZE_BOOL` of a double (0 for NaN).
fn normalize(value: f64) -> i32 {
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}

/// `zendi_smart_strcmp`.
fn smart_strcmp(a: &[u8], b: &[u8]) -> i32 {
    ZString::new(a).smart_cmp(&ZString::new(b))
}

/// `zend_binary_strcmp`, normalized: the bytes in order, a prefix first.
fn binary_strcmp(a: &[u8], b: &[u8]) -> i32 {
    sign(a.cmp(b))
}

/// Upstream's `compare_long_to_string`.
fn long_to_string(lval: i64, text: &[u8]) -> i32 {
    match numeric(text) {
        (kind, number, _) if kind == IS_LONG.c_cast::<u8>() => sign(lval.cmp(&number)),
        (kind, _, number) if kind == IS_DOUBLE.c_cast::<u8>() => {
            normalize(lval.c_cast::<f64>() - number)
        }
        _ => binary_strcmp(lval.to_string().as_bytes(), text),
    }
}

/// Upstream's `compare_double_to_string`.
fn double_to_string(dval: f64, text: &[u8]) -> i32 {
    match numeric(text) {
        (kind, number, _) if kind == IS_LONG.c_cast::<u8>() => {
            normalize(dval - number.c_cast::<f64>())
        }
        #[expect(clippy::float_cmp, reason = "upstream compares with ==")]
        (kind, _, number) if kind == IS_DOUBLE.c_cast::<u8>() => {
            if dval == number {
                0
            } else {
                normalize(dval - number)
            }
        }
        _ => {
            let string = ZString::from_double(dval);
            binary_strcmp(string.bytes(), text)
        }
    }
}
