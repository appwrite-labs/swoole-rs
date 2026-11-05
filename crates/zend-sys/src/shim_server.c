#include "shim.h"
#include "shim_server.h"
#include "ext/standard/php_var.h"
#include "zend_smart_str.h"

zend_string *swr_zend_serialize(zval *value, bool *bailed) {
    smart_str buffer = {0};
    SWR_ZEND_TRY {
        php_serialize_data_t var_hash;
        PHP_VAR_SERIALIZE_INIT(var_hash);
        php_var_serialize(&buffer, value, &var_hash);
        PHP_VAR_SERIALIZE_DESTROY(var_hash);
    } SWR_ZEND_CATCH {
        *bailed = true;
    } SWR_ZEND_END_TRY();
    if (*bailed) {
        smart_str_free(&buffer);
        return NULL;
    }
    return buffer.s;
}

bool swr_zend_unserialize(zval *out, const char *data, size_t length, size_t *offset, bool *bailed) {
    const unsigned char *cursor = (const unsigned char *) data;
    bool done = false;
    SWR_ZEND_TRY {
        php_unserialize_data_t var_hash;
        PHP_VAR_UNSERIALIZE_INIT(var_hash);
        done = php_var_unserialize(out, &cursor, (const unsigned char *) data + length, &var_hash);
        PHP_VAR_UNSERIALIZE_DESTROY(var_hash);
    } SWR_ZEND_CATCH {
        *bailed = true;
    } SWR_ZEND_END_TRY();
    *offset = (size_t) ((const char *) cursor - data);
    return done;
}
