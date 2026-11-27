//! The Swoole\SendFile descriptor carries options without owning the named file.
use std::cell::Cell;
use swoole_rs_phpstubs::sendfile::arginfo_class_Swoole_SendFile___construct;
use swoole_rs_zend::{Call, Custom, CustomClass, Minit, Owned, handlers, method_table, sys as z};

#[derive(Clone, Copy, PartialEq)]
enum State {
    New,
    Ready,
    Submitted,
}

struct Descriptor(Cell<State>);

/// The arguments of a PHP sendfile call, retained across hooks and coroutine suspension.
pub struct SendFile<'a> {
    pub filename: swoole_rs_zend::ZString,
    pub offset: i64,
    pub length: i64,
    pub delete: bool,
    descriptor: Option<&'a Descriptor>,
}

const SUBMITTED: &std::ffi::CStr =
    c"Swoole\\SendFile has already transferred file cleanup ownership";

impl<'a> SendFile<'a> {
    /// Parse the filename argument and the three optional arguments that follow it.
    #[must_use]
    pub fn parse(call: &'a Call<'_>, index: u32) -> Option<Self> {
        call.arity(index, index + 3).ok()?;
        let value = call.value(index)?;
        let descriptor = value.custom::<Descriptor>();
        let path = if descriptor.is_none() {
            Some(call.zstr(index).ok()??.to_owned())
        } else {
            None
        };
        let offset = call.long(index + 1, 0).ok()?;
        let length = call.long(index + 2, 0).ok()?;
        let delete = call.bool(index + 3, false).ok()?;
        if let Some(filename) = path {
            if filename.bytes().contains(&0) {
                call.argument_value_error(index, c"must not contain any null bytes");
                return None;
            }
            return Some(Self {
                filename,
                offset,
                length,
                delete,
                descriptor,
            });
        }
        if call.num_args() > index {
            call.argument_value_error(
                index + 1,
                c"must not be provided when $filename is a Swoole\\SendFile",
            );
            return None;
        }
        match descriptor?.0.get() {
            State::New => {
                call.throw_error(c"Swoole\\SendFile constructor must be called first");
                return None;
            }
            State::Submitted => {
                call.throw_error(SUBMITTED);
                return None;
            }
            State::Ready => {}
        }
        let props = value.props(Some(&CLASS))?;
        let offset = props.slot(c"offset")?.as_long()?;
        let length = props.slot(c"length")?.as_long()?;
        if offset < 0 || length < 0 {
            swoole_rs_zend::value_error(if offset < 0 {
                c"Swoole\\SendFile::$offset must be greater than or equal to 0"
            } else {
                c"Swoole\\SendFile::$length must be greater than or equal to 0"
            });
            return None;
        }
        Some(Self {
            filename: props.slot(c"filename")?.str()?.to_owned(),
            offset,
            length,
            delete: props.slot(c"deleteAfterSend")?.truthy(),
            descriptor,
        })
    }

    /// Transfer cleanup after validation. A reentrant send may already own it.
    #[must_use]
    pub fn acquire(&self) -> bool {
        if self.delete
            && let Some(object) = self.descriptor
            && object.0.replace(State::Submitted) == State::Submitted
        {
            swoole_rs_zend::throw_error(SUBMITTED);
            return false;
        }
        true
    }
}

static CLASS: CustomClass<Descriptor> = CustomClass::new();

impl Custom for Descriptor {
    fn class() -> &'static CustomClass<Self> {
        &CLASS
    }

    fn new() -> Self {
        Self(Cell::new(State::New))
    }
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_sendfile___construct, &arginfo_class_Swoole_SendFile___construct, z::ZEND_ACC_PUBLIC;
    ];
}

pub(crate) fn register(minit: &Minit) {
    minit.register_object::<Descriptor>(c"Swoole\\SendFile", METHODS.table(), z::ZEND_ACC_FINAL);
    let public = z::ZEND_ACC_PUBLIC;
    minit.scalar_property(
        &CLASS,
        c"filename",
        &Owned::empty(),
        public | z::ZEND_ACC_READONLY,
        z::MAY_BE_STRING,
    );
    for name in [c"offset", c"length"] {
        minit.scalar_property(&CLASS, name, &Owned::long(0), public, z::MAY_BE_LONG);
    }
    minit.scalar_property(
        &CLASS,
        c"deleteAfterSend",
        &Owned::bool(false),
        public,
        z::MAY_BE_BOOL,
    );
}

fn construct(call: &Call) {
    let Ok(Some(filename)) = call.arity(1, 1).and_then(|()| call.zstr(1)) else {
        return;
    };
    if filename.bytes().contains(&0) {
        return call.argument_value_error(1, c"must not contain any null bytes");
    }
    let Some(descriptor) = call.custom::<Descriptor>() else {
        return;
    };
    if descriptor.0.get() != State::New {
        return call.throw_constructed_twice();
    }
    if filename.bytes().is_empty() {
        return call.argument_value_error(1, c"must not be empty");
    }
    if let Some(props) = call.props(Some(&CLASS)) {
        props.value(c"filename", &Owned::string(filename.to_owned()));
        descriptor.0.set(State::Ready);
    }
}

handlers! { zim_swoole_sendfile___construct => construct; }

/// Warn before either PHP sendfile method dispatches a negative file range.
#[must_use]
pub fn valid_sendfile_range(offset: i64, length: i64) -> bool {
    for (parameter, value) in [("offset", offset), ("length", length)] {
        if value < 0 {
            swoole_rs_corebase::log::set_last_error(
                swoole_rs_error::ErrorCode::SW_ERROR_PHP_RUNTIME_NOTICE as i32,
            );
            if crate::display_errors() {
                let warning = swoole_rs_zend::message!(
                    "parameter ${parameter}[{value}] must be greater than or equal to 0"
                );
                swoole_rs_zend::bailout::defer(swoole_rs_zend::report(
                    swoole_rs_zend::E_WARNING,
                    warning.as_cstr(),
                ));
            }
            return false;
        }
    }
    true
}
