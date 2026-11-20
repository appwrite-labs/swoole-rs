//! Result-column metadata for the bundled PDO SQLite statement.
use std::ffi::c_int;
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::{
    Owned, OwnedArray, ZStr,
    pdo::Slot,
    sys::{
        _zend_known_string_id_ZEND_STR_DOUBLE as DOUBLE,
        _zend_known_string_id_ZEND_STR_INTEGER as INTEGER,
        _zend_known_string_id_ZEND_STR_NULL_LOWERCASE as NULL_NAME,
        _zend_known_string_id_ZEND_STR_STRING as STRING, pdo_param_type_PDO_PARAM_INT as INT,
        pdo_param_type_PDO_PARAM_NULL as NULL, pdo_param_type_PDO_PARAM_STR as STR,
    },
};

#[unsafe(no_mangle)]
extern "C" fn swr_sqlite_column_meta(
    output: &Slot,
    kind: c_int,
    declared: Option<Text<'_>>,
    table: Option<Text<'_>>,
) {
    unsafe extern "C" {
        /// Called only with Zend's known string IDs below.
        safe fn swr_sqlite_known_type(id: u32) -> &'static ZStr;
    }
    let mut meta = OwnedArray::default();
    let mut flags = OwnedArray::default();
    let types = match kind {
        1 => Some((INTEGER, INT)),
        2 => Some((DOUBLE, STR)),
        3 => Some((STRING, STR)),
        4 => {
            flags.push_bytes(b"blob");
            Some((STRING, STR))
        }
        5 => Some((NULL_NAME, NULL)),
        _ => None,
    };
    if let Some((name, param)) = types {
        meta.insert(
            b"native_type",
            Owned::string(swr_sqlite_known_type(name).to_owned()),
        );
        meta.insert_long(b"pdo_type", param.into());
    }
    if let Some(declared) = declared {
        meta.insert_bytes(b"sqlite:decl_type", Some(declared.get().to_bytes()));
    }
    if let Some(table) = table {
        meta.insert_bytes(b"table", Some(table.get().to_bytes()));
    }
    meta.insert(b"flags", flags.into_owned());
    output.set(meta.into_owned());
}
