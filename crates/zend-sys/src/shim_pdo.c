/* Stays C (docs/zend.md, What stays C): private (pdo_dbh_t, from ext/pdo headers
 * bindgen does not read), version (REGISTER_PDO_CLASS_CONST_LONG_DEPRECATED_85) and
 * globals (EG(persistent_list)). */
#include "shim_pdo.h"
#include "ext/pdo/php_pdo.h"
#include "ext/pdo/php_pdo_driver.h"

bool swr_zend_pdo_declared(const char *name) {
    return zend_hash_str_find(&php_pdo_get_dbh_ce()->constants_table, name, strlen(name)) != NULL;
}

void swr_zend_pdo_declare_long(const char *name, zend_long value) {
#if PHP_VERSION_ID >= 80500
    REGISTER_PDO_CLASS_CONST_LONG_DEPRECATED_85(name, value);
#else
    zend_declare_class_constant_long(php_pdo_get_dbh_ce(), name, strlen(name), value);
#endif
}

void *swr_zend_pdo_driver_data(void *dbh) {
    return ((pdo_dbh_t *) dbh)->driver_data;
}

void swr_zend_pdo_forget_persistent(void *handle) {
    pdo_dbh_t *dbh = handle;
    if (!dbh->is_persistent || !dbh->persistent_id) {
        return;
    }
    zend_resource *resource =
        zend_hash_str_find_ptr(&EG(persistent_list), dbh->persistent_id, dbh->persistent_id_len);
    if (resource && resource->ptr == dbh) {
        resource->ptr = NULL;
        ZEND_ASSERT(dbh->refcount > 1);
        dbh->refcount--;
        zend_hash_str_del(&EG(persistent_list), dbh->persistent_id, dbh->persistent_id_len);
    }
}
