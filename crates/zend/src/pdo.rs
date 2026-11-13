//! PDO's side of a bundled driver's MINIT: the `PDO` class's constants and the
//! driver table, behind [`Minit::pdo`], which checks PDO's own MINIT has run.
use crate::{
    Minit, Mshutdown,
    sys::{swr_zend_pdo_declare_long, swr_zend_pdo_declared, swr_zend_pdo_loaded, zend_long},
};
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
    /// PDO's `php_pdo_register_driver`: `SUCCESS` (0) or `FAILURE` (-1).
    /// # Safety
    /// MINIT, on the main thread. A driver built for another PDO API, or PDO
    /// not loaded, raises `E_ERROR`, which bails out over the caller's frames:
    /// they own nothing that needs a drop.
    pub fn php_pdo_register_driver(driver: &'static Driver) -> c_int;
    /// PDO's `php_pdo_unregister_driver`: nothing once PDO has shut down, or
    /// when it is not loaded. It raises no error.
    fn php_pdo_unregister_driver(driver: &'static Driver);
}

/// PDO, loaded, during this module's MINIT: what makes its class and driver
/// table safe to change.
pub struct Pdo<'a>(PhantomData<&'a Minit>);

impl Minit {
    /// PDO, once its MINIT has registered the `PDO` class; `None` when PDO is
    /// not loaded, or starts after this module.
    #[must_use]
    pub fn pdo(&self) -> Option<Pdo<'_>> {
        // SAFETY: MINIT; reads PDO's class pointer.
        unsafe { swr_zend_pdo_loaded() }.then_some(Pdo(PhantomData))
    }
}

impl Mshutdown {
    /// `php_pdo_unregister_driver`: PDO, if it is still up, forgets `driver`'s name.
    pub fn pdo_unregister(&self, driver: &'static Driver) {
        // SAFETY: MSHUTDOWN on the main thread, per the token; PDO checks its
        // own module is still registered before it touches its driver table.
        unsafe { php_pdo_unregister_driver(driver) };
    }
}

impl Pdo<'_> {
    /// Whether the `PDO` class declares constant `name`.
    #[must_use]
    pub fn declared(&self, name: &CStr) -> bool {
        // SAFETY: MINIT, with the `PDO` class registered.
        unsafe { swr_zend_pdo_declared(name.as_ptr()) }
    }

    /// `REGISTER_PDO_CLASS_CONST_LONG` (deprecated from PHP 8.5, as PDO's own
    /// drivers'): nothing when the class has `name` already or `name` is
    /// `class`, where Zend's would raise a fatal error.
    pub fn declare_long(&self, name: &CStr, value: zend_long) {
        // SAFETY: MINIT, with the `PDO` class registered; the shim declares
        // only what Zend accepts, so nothing bails out. Zend copies the name.
        unsafe { swr_zend_pdo_declare_long(name.as_ptr(), value) };
    }

    /// `php_pdo_unregister_driver`: PDO forgets `driver`'s name, whichever
    /// driver registered it.
    pub fn unregister(&self, driver: &'static Driver) {
        // SAFETY: MINIT on the main thread, per the token.
        unsafe { php_pdo_unregister_driver(driver) };
    }
}
