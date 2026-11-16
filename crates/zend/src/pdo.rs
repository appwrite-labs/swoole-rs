//! PDO's side of a bundled driver's MINIT: the `PDO` class's constants and the
//! driver table, behind [`Minit::pdo`], which checks PDO's own MINIT has run.
use crate::{Minit, Mshutdown, bailout::Bailout, helpers::CText, sys::zend_long};
use std::{
    ffi::{CStr, c_int},
    marker::PhantomData,
};

/// A `pdo_driver_t`, opaque. A driver crate declares its own as a `safe static`
/// of this type in an `unsafe extern` block, which is what vouches for it:
/// nothing else makes one.
#[repr(C)]
pub struct Driver {
    _opaque: [u8; 0],
}

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
