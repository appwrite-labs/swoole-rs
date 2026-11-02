#include "shim_http_ctx.h"
#include "SAPI.h"

uint32_t swr_zend_http_property_offset(zend_class_entry *ce, const char *name, size_t length) {
    zend_property_info *info = zend_hash_str_find_ptr(&ce->properties_info, name, length);
    return info ? info->offset : 0;
}

zval *swr_zend_http_property_array(zend_object *object, uint32_t offset, uint32_t size) {
    zval *property = OBJ_PROP(object, offset);
    array_init_size(property, size);
    return property;
}

void swr_zend_http_add_or_merge(zval *array, const char *key, size_t length, zval *element) {
    zend_ulong index;
    HashTable *ht = Z_ARRVAL_P(array);
    int numeric = ZEND_HANDLE_NUMERIC_STR(key, length, index);
    zval *added = numeric ? zend_hash_index_add(ht, index, element) : zend_hash_str_add(ht, key, length, element);
    if (added) {
        return;
    }
    zval *current = numeric ? zend_hash_index_find(ht, index) : zend_hash_str_find(ht, key, length);
    if (Z_TYPE_P(current) == IS_ARRAY) {
        add_next_index_zval(current, element);
    } else {
        zval list;
        array_init_size(&list, 2);
        Z_ADDREF_P(current);
        add_next_index_zval(&list, current);
        add_next_index_zval(&list, element);
        if (numeric) {
            zend_hash_index_update(ht, index, &list);
        } else {
            zend_hash_str_update(ht, key, length, &list);
        }
    }
}

char *swr_zend_http_estrndup(const char *text, size_t length) {
    return estrndup(text, length);
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

void swr_zend_http_get_string(void *arg) {
    swr_zend_http_string *string = arg;
    string->result = zval_get_string(string->value);
}
