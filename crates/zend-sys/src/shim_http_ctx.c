/* Stays C (docs/zend.md, What stays C): bailout (trap bodies that run
 * __toString or __destruct), globals (PG, SG(rfc1867_uploaded_files)),
 * allocator (estrndup, array_init_size, zend_string_*) and version
 * (PG(arg_separator) became a zend_string in 8.5). */
#include "shim_http_ctx.h"
#include "SAPI.h"
#include "main/php_variables.h"
#include "main/rfc1867.h"
#include "ext/standard/url.h"

#include <unistd.h>

uint32_t swr_zend_http_property_offset(zend_class_entry *ce, const char *name) {
    zend_property_info *info = zend_hash_str_find_ptr(&ce->properties_info, name, strlen(name));
    return info ? info->offset : 0;
}

zval *swr_zend_http_property_array(zend_object *object, uint32_t offset, uint32_t size) {
    zval *property = OBJ_PROP(object, offset);
    array_init_size(property, size);
    return property;
}

zend_string *swr_zend_http_intern(const char *text, size_t length) {
    return zend_string_init_interned(text, length, 1);
}

zend_long swr_zend_http_max_input_vars(void) {
    return PG(max_input_vars);
}

void swr_zend_http_upload(zval *tmpfiles, const char *file, size_t length) {
    zend_string *path = zend_string_init(file, length, 0);
    add_next_index_str(tmpfiles, zend_string_copy(path));
    zend_hash_add_ptr(SG(rfc1867_uploaded_files), path, path);
    zend_string_release(path);
}

void swr_zend_http_release_objects(void *arg) {
    swr_zend_http_release *release = arg;
    if (release->begun == 0) {
        release->begun = 1;
        zval_ptr_dtor(release->request);
    }
    release->begun = 2;
    zval_ptr_dtor(release->response);
}

bool swr_zend_http_treat_data(int arg, char *str, zval *dest) {
    zval array;
    char *var, *val, *separator = NULL;
    char *strtok_buf = NULL;
    zend_long count = 0;
    bool within = true;

    ZVAL_COPY_VALUE(&array, dest);
    if (!str) {
        return true;
    }
    switch (arg) {
    case PARSE_STRING:
#if PHP_VERSION_ID >= 80500
        separator = ZSTR_VAL(PG(arg_separator).input);
#else
        separator = PG(arg_separator).input;
#endif
        break;
    case PARSE_COOKIE:
        separator = (char *) ";\0";
        break;
    }

    var = php_strtok_r(str, separator, &strtok_buf);
    while (var) {
        size_t val_len;
        size_t new_val_len;

        val = strchr(var, '=');
        if (arg == PARSE_COOKIE) {
            /* Leading spaces of a cookie name, as after "; " in a multi-cookie header. */
            while (isspace(*var)) {
                var++;
            }
            if (var == val || *var == '\0') {
                goto next_cookie;
            }
        }
        if (++count > PG(max_input_vars)) {
            within = false;
            break;
        }
        if (val) {
            *val++ = '\0';
            val_len = arg == PARSE_COOKIE ? php_raw_url_decode(val, strlen(val)) : php_url_decode(val, strlen(val));
        } else {
            val = (char *) "";
            val_len = 0;
        }
        val = estrndup(val, val_len);
        if (arg != PARSE_COOKIE) {
            php_url_decode(var, strlen(var));
        }
        if (sapi_module.input_filter(PARSE_STRING, var, &val, val_len, &new_val_len)) {
            if (arg == PARSE_STRING || !zend_symtable_str_exists(Z_ARRVAL(array), var, strlen(var))) {
                php_register_variable_safe(var, val, new_val_len, &array);
            }
        }
        efree(val);
    next_cookie:
        var = php_strtok_r(NULL, separator, &strtok_buf);
    }
    efree(str);
    return within;
}

void swr_zend_http_uploads_release(zend_object *object, uint32_t offset) {
    zval *path;
    zval *tmpfiles = OBJ_PROP(object, offset);
    ZVAL_DEREF(tmpfiles);
    if (Z_TYPE_P(tmpfiles) != IS_ARRAY) {
        return;
    }
    ZEND_HASH_FOREACH_VAL(Z_ARRVAL_P(tmpfiles), path) {
        if (Z_TYPE_P(path) != IS_STRING) {
            continue;
        }
        unlink(Z_STRVAL_P(path));
        if (SG(rfc1867_uploaded_files)) {
            zend_hash_str_del(SG(rfc1867_uploaded_files), Z_STRVAL_P(path), Z_STRLEN_P(path));
        }
    }
    ZEND_HASH_FOREACH_END();
}

void swr_zend_http_uploads_rinit(void) {
    if (!SG(rfc1867_uploaded_files)) {
        ALLOC_HASHTABLE(SG(rfc1867_uploaded_files));
        zend_hash_init(SG(rfc1867_uploaded_files), 8, NULL, NULL, 0);
    }
}

void swr_zend_http_uploads_rshutdown(void) {
    if (SG(rfc1867_uploaded_files)) {
        destroy_uploaded_files_hash();
        SG(rfc1867_uploaded_files) = NULL;
    }
}
