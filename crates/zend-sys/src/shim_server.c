/* Stays C (docs/zend.md, What stays C): bailout (serialize, unserialize and array
 * merges run PHP), globals (EG(stack_limit), PG(last_error_*)) and version
 * (ZEND_CHECK_STACK_LIMIT). */
#include "shim.h"
#include "shim_server.h"
#include "main/php_globals.h"
#include "ext/standard/php_array.h"
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

int swr_zend_try_array_merge(zval *dest, zval *src) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        SEPARATE_ARRAY(dest);
        php_array_merge(Z_ARRVAL_P(dest), Z_ARRVAL_P(src));
    } SWR_ZEND_CATCH {
        bailed = 1;
    } SWR_ZEND_END_TRY();
    return bailed;
}

void swr_zend_stack_unlimit(void **saved) {
#ifdef ZEND_CHECK_STACK_LIMIT
    saved[0] = EG(stack_limit);
    saved[1] = EG(stack_base);
    EG(stack_base) = (void *) 0;
    EG(stack_limit) = (void *) 0;
#else
    (void) saved;
#endif
}

void swr_zend_stack_restore(void *const *saved) {
#ifdef ZEND_CHECK_STACK_LIMIT
    EG(stack_limit) = saved[0];
    EG(stack_base) = saved[1];
#else
    (void) saved;
#endif
}

bool swr_zend_mkdir_recursive(const char *path) {
    return php_stream_mkdir(path, 0777, PHP_STREAM_MKDIR_RECURSIVE | REPORT_ERRORS, NULL);
}

bool swr_zend_last_fatal_error(const char **message, const char **file, int *line) {
    if (!PG(last_error_message) || !(PG(last_error_type) & E_FATAL_ERRORS)) {
        return false;
    }
    *message = ZSTR_VAL(PG(last_error_message));
    *file = PG(last_error_file) ? ZSTR_VAL(PG(last_error_file)) : "-";
    *line = PG(last_error_lineno);
    return true;
}
