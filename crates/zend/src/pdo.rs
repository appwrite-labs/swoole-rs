//! PDO's side of a bundled driver: at MINIT the `PDO` class's constants and the
//! driver table, behind [`Minit::pdo`], which checks PDO's own MINIT has run; at
//! run time the database handle ([`Dbh`]), statement ([`Stmt`]) and value slots
//! ([`Slot`]) PDO lends the driver's methods.
use crate::{
    Minit, Mshutdown, Owned, Value,
    bailout::Bailout,
    helpers::CText,
    sys::{pdo_dbh_methods, pdo_dbh_t, pdo_error_type, pdo_stmt_t, zend_long},
};
use std::{
    cell::Cell,
    ffi::{CStr, c_char, c_int, c_void},
    marker::PhantomData,
};

/// A native `pdo_driver_t` registered with PDO. The driver supplies its
/// immutable descriptor, including the factory pointer and header API version.
#[repr(transparent)]
pub struct Driver(crate::sys::pdo_driver_t);

unsafe extern "C" {
    /// Whether PDO has registered its class: it reads one pointer PDO's MINIT
    /// wrote.
    safe fn swr_zend_pdo_loaded() -> bool;
}

// PDO's class constants and driver table, which change only at MINIT and
// MSHUTDOWN, on the main thread. Each is total and runs no PHP code. Private, so
// only the methods of a [`Pdo`] or an [`Mshutdown`], the tokens of those two
// phases, call them.
unsafe extern "C" {
    /// PDO's `php_pdo_register_driver`: `SUCCESS` (0) or `FAILURE` (-1). Unlike the
    /// others it is not total: a driver built for another PDO API raises `E_ERROR`,
    /// which may bail out, so only [`Pdo::register`] calls it, inside `zend_try`.
    safe fn php_pdo_register_driver(driver: &'static Driver) -> c_int;
    /// PDO's `php_pdo_unregister_driver`: nothing once PDO has shut down, or
    /// when it is not loaded. It raises no error.
    safe fn php_pdo_unregister_driver(driver: &'static Driver);
    /// PDO's `pdo_throw_exception`: a `PDOException` with `sqlstate`, `code` and
    /// `message`, which PDO reads and copies. It creates the exception object and
    /// sets its properties, which runs no PHP code.
    safe fn pdo_throw_exception(code: u32, message: CText<'_>, sqlstate: &pdo_error_type);
    /// PDO's `php_pdo_get_exception`: the `PDOException` class, which PDO's MINIT
    /// registered and which lives for the process.
    safe fn php_pdo_get_exception() -> Option<crate::ClassEntry<'static>>;
    /// PDO's `pdo_get_bool_param`: the value as an attribute's bool, or false with a
    /// `TypeError` thrown, which runs no PHP code.
    safe fn pdo_get_bool_param(bval: &mut bool, value: &Value) -> bool;
    /// Whether the `PDO` class declares constant `name`; false when PDO has not
    /// registered the class.
    safe fn swr_zend_pdo_declared(name: CText<'_>) -> bool;
    /// `REGISTER_PDO_CLASS_CONST_LONG`: false, declaring nothing, when PDO has
    /// not registered its class, the class has `name` already or `name` is
    /// `class` (where Zend's would raise a fatal error). Zend copies the name.
    safe fn swr_zend_pdo_declare_long(name: CText<'_>, value: zend_long) -> bool;
}

/// PDO, loaded, during this module's MINIT: what makes its class and driver
/// table safe to change.
pub struct Pdo<'a>(PhantomData<&'a Minit>);

impl Minit {
    /// PDO, once its MINIT has registered the `PDO` class; `None` when PDO is
    /// not loaded, or starts after this module.
    #[must_use]
    pub fn pdo(&self) -> Option<Pdo<'_>> {
        swr_zend_pdo_loaded().then_some(Pdo(PhantomData))
    }
}

impl Mshutdown {
    /// `php_pdo_unregister_driver`: PDO, if it is still up, forgets `driver`'s name.
    pub fn pdo_unregister(&self, driver: &'static Driver) {
        php_pdo_unregister_driver(driver);
    }
}

impl Pdo<'_> {
    /// Whether the `PDO` class declares constant `name`.
    #[must_use]
    pub fn declared(&self, name: &CStr) -> bool {
        swr_zend_pdo_declared(name.into())
    }

    /// `REGISTER_PDO_CLASS_CONST_LONG` (deprecated from PHP 8.5, as PDO's own
    /// drivers'): nothing when the class has `name` already or `name` is
    /// `class`, where Zend's would raise a fatal error.
    pub fn declare_long(&self, name: &CStr, value: zend_long) {
        swr_zend_pdo_declare_long(name.into(), value);
    }

    /// `php_pdo_unregister_driver`: PDO forgets `driver`'s name, whichever
    /// driver registered it.
    pub fn unregister(&self, driver: &'static Driver) {
        php_pdo_unregister_driver(driver);
    }

    /// `php_pdo_register_driver` inside `zend_try`: PDO adds `driver` under its
    /// name, unless the name is taken.
    /// # Errors
    /// PDO raised `E_ERROR` (a driver built for another PDO API) and bailed out.
    pub fn register(&self, driver: &'static Driver) -> Result<(), Bailout> {
        crate::contain_with(|| {
            php_pdo_register_driver(driver);
        })
    }
}

/// A database handle PDO lends a driver's factory or method: its live `pdo_dbh_t`,
/// whose driver data is the driver's. PHP code a method runs (a notice callback, an
/// error handler) may reach the same handle, so fields are copied in and out, never
/// borrowed. Only a driver export's parameter makes one (`&Dbh`): PDO's call stands
/// for the constructor.
#[repr(transparent)]
pub struct Dbh(Cell<pdo_dbh_t>);

impl Dbh {
    /// `f` of a copy of the fields, then stored back: `f` runs no PHP code.
    fn update<R>(&self, f: impl FnOnce(&mut pdo_dbh_t) -> R) -> R {
        let mut dbh = self.0.get();
        let result = f(&mut dbh);
        self.0.set(dbh);
        result
    }

    /// Whether the driver has installed its methods: PDO's constructor is done
    /// with the factory.
    #[must_use]
    pub fn has_methods(&self) -> bool {
        !self.0.get().methods.is_null()
    }

    /// PDO's configured fetch mode for methods called with `PDO::FETCH_DEFAULT`.
    #[must_use]
    pub fn default_fetch_type(&self) -> crate::sys::pdo_fetch_type {
        #[cfg(php85)]
        {
            self.0.get().default_fetch_type.into()
        }
        #[cfg(not(php85))]
        {
            self.0.get().default_fetch_type
        }
    }

    /// `dbh->methods = methods`.
    pub fn set_methods(&self, methods: &'static pdo_dbh_methods) {
        self.update(|dbh| dbh.methods = methods);
    }

    /// `dbh->driver_data = data`.
    pub fn set_driver_data(&self, data: *mut c_void) {
        self.update(|dbh| dbh.driver_data = data);
    }

    /// `dbh->error_code`: the handle's SQLSTATE.
    #[must_use]
    pub fn error_code(&self) -> pdo_error_type {
        self.0.get().error_code
    }

    /// `strcpy(dbh->error_code, sqlstate)`.
    pub fn set_error_code(&self, sqlstate: &CStr) {
        self.update(|dbh| copy_state(&mut dbh.error_code, sqlstate));
    }

    /// Whether PDO requested autocommit on this connection.
    #[must_use]
    pub fn auto_commit(&self) -> bool {
        #[cfg(php85)]
        {
            self.0.get().auto_commit()
        }
        #[cfg(not(php85))]
        {
            self.0.get().auto_commit() != 0
        }
    }

    /// Set PDO's autocommit flag before the driver applies the change.
    pub fn set_auto_commit(&self, on: bool) {
        self.update(|dbh| {
            #[cfg(php85)]
            dbh.set_auto_commit(on);
            #[cfg(not(php85))]
            dbh.set_auto_commit(on.into());
        });
    }

    /// `dbh->is_closed = 1`: the driver found the connection gone.
    pub fn set_closed(&self) {
        self.update(|dbh| {
            #[cfg(php85)]
            dbh.set_is_closed(true);
            #[cfg(not(php85))]
            dbh.set_is_closed(1);
        });
    }

    /// Whether PDO is tracking an active transaction.
    #[must_use]
    pub fn in_txn(&self) -> bool {
        self.0.get().in_txn()
    }

    /// `dbh->in_txn = in_txn`.
    pub fn set_in_txn(&self, in_txn: bool) {
        self.update(|dbh| dbh.set_in_txn(in_txn));
    }

    /// The flags a factory sets: the parameter events PDO need not dispatch, that
    /// the driver allocates its own column names, and how long one character's
    /// quoted form can grow.
    pub fn set_driver_flags(&self, skip_param_evt: u8, max_escaped_char_length: u8) {
        self.update(|dbh| {
            #[cfg(php85)]
            {
                dbh.set_alloc_own_columns(true);
                dbh.skip_param_evt = skip_param_evt;
                dbh.max_escaped_char_length = max_escaped_char_length;
            }
            #[cfg(not(php85))]
            {
                dbh.set_alloc_own_columns(1);
                dbh.set_skip_param_evt(skip_param_evt.into());
                dbh.set_max_escaped_char_length(max_escaped_char_length.into());
            }
        });
    }

    /// `pdo_throw_exception(code, message, &dbh->error_code)`.
    pub fn throw(&self, code: u32, message: &CStr) {
        pdo_throw_exception(code, message.into(), &self.error_code());
    }

    /// `zend_throw_exception_ex(php_pdo_get_exception(), code, "%s", message)`:
    /// a `PDOException` with only a message and code, as a driver's constructor
    /// throws before PDO's error handling is in place. A bailout (no frame
    /// executing) is deferred.
    pub fn throw_exception(&self, code: zend_long, message: &CStr) {
        crate::bailout::defer(crate::error::throw_entry(
            php_pdo_get_exception(),
            message,
            code,
        ));
    }
}

/// A statement PDO lends a driver, as [`Dbh`] is lent: only a driver export's
/// parameter makes one.
#[repr(transparent)]
pub struct Stmt(Cell<pdo_stmt_t>);

impl Stmt {
    /// Number of rows PDO exposes for the current result.
    pub fn row_count(&self) -> zend_long {
        self.0.get().row_count
    }

    /// Update the row count for a new batch of the current result.
    pub fn set_row_count(&self, rows: zend_long) {
        let mut stmt = self.0.get();
        stmt.row_count = rows;
        self.0.set(stmt);
    }

    /// Publish the dimensions of the newly executed statement to PDO.
    pub fn set_result_counts(&self, columns: i32, rows: zend_long) {
        let mut stmt = self.0.get();
        stmt.column_count = columns;
        stmt.row_count = rows;
        self.0.set(stmt);
    }

    /// `stmt->error_code`: the statement's SQLSTATE.
    #[must_use]
    pub fn error_code(&self) -> pdo_error_type {
        self.0.get().error_code
    }

    /// `strcpy(stmt->error_code, sqlstate)`.
    pub fn set_error_code(&self, sqlstate: &CStr) {
        let mut stmt = self.0.get();
        copy_state(&mut stmt.error_code, sqlstate);
        self.0.set(stmt);
    }
}

/// `strcpy` of `sqlstate`, at most five characters, into an error code.
fn copy_state(code: &mut pdo_error_type, sqlstate: &CStr) {
    *code = [0; 6];
    for (to, &from) in code.iter_mut().zip(sqlstate.to_bytes().iter().take(5)) {
        *to = c_char::from_ne_bytes([from]);
    }
}

/// A value PDO lends a driver method to fill: `getAttribute`'s return value, or
/// `errorInfo`'s array. Only a driver export's parameter makes one (`&Slot`).
#[repr(transparent)]
pub struct Slot(Value);

impl Slot {
    /// The current value of this PDO slot.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.0
    }

    /// The slot holds `value`, releasing what it held.
    pub fn set(&self, value: Owned) {
        self.0.lend_slot(|held| *held = value);
    }

    /// Append driver-specific fields to the error information array lent by PDO.
    pub fn edit_array(&self, edit: impl FnOnce(&mut crate::ArrayMut)) {
        self.0.edit_array_slot(edit);
    }

    /// `add_next_index_long` of `code`, or `add_next_index_null`, then
    /// `add_next_index_string` of `message` if any: a driver's `errorInfo` fields.
    pub fn push_error_info(&self, code: Option<u32>, message: Option<&[u8]>) {
        self.edit_array(|info| {
            match code {
                Some(code) => info.push_long(code.into()),
                None => info.push_null(),
            }
            if let Some(message) = message {
                info.push_bytes(message);
            }
        });
    }
}

/// `get_attribute`'s answer, as PDO reads its `int`.
#[repr(i32)]
#[derive(Clone, Copy)]
pub enum Attribute {
    Unknown = 0,
    Set = 1,
}

/// `pdo_attr_lval(options, attribute, default)`: the driver option `attribute` as
/// an integer, converted as PHP does, or `default`.
#[must_use]
pub fn attr_long(options: Option<&Value>, attribute: u32, default: zend_long) -> zend_long {
    options
        .and_then(Value::array)
        .and_then(|options| options.index(attribute.into()))
        .map_or(default, Value::long)
}

/// PDO's integer option parser, including its type error for an invalid value.
#[must_use]
pub fn long_param(value: &Value) -> Option<zend_long> {
    unsafe extern "C" {
        safe fn pdo_get_long_param(output: &mut zend_long, value: &Value) -> bool;
    }
    let mut output = 0;
    pdo_get_long_param(&mut output, value).then_some(output)
}

/// `pdo_get_bool_param`: the value as an attribute's bool, or none with a
/// `TypeError` thrown.
#[must_use]
pub fn bool_param(value: &Value) -> Option<bool> {
    let mut bval = false;
    pdo_get_bool_param(&mut bval, value).then_some(bval)
}
