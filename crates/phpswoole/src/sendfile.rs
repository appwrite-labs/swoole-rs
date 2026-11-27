//! The Swoole\SendFile descriptor carries options without owning the named file.
use std::cell::Cell;
use swoole_rs_phpstubs::sendfile::arginfo_class_Swoole_SendFile___construct;
use swoole_rs_zend::{Call, Custom, CustomClass, Minit, Owned, handlers, method_table, sys as z};

struct Descriptor(Cell<bool>);

static CLASS: CustomClass<Descriptor> = CustomClass::new();

impl Custom for Descriptor {
    fn class() -> &'static CustomClass<Self> {
        &CLASS
    }

    fn new() -> Self {
        Self(Cell::new(false))
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
    if descriptor.0.get() {
        return call.throw_constructed_twice();
    }
    if filename.bytes().is_empty() {
        return call.argument_value_error(1, c"must not be empty");
    }
    if let Some(props) = call.props(Some(&CLASS)) {
        props.value(c"filename", &Owned::string(filename.to_owned()));
        descriptor.0.set(true);
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
