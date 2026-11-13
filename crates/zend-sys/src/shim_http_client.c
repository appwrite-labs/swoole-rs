/* Stays C (docs/zend.md, What stays C): globals (EG(uninitialized_zval)), allocator
 * (array_init, smart_str), version (php_url_encode_hash_ex's
 * signature before 8.3, zend_is_true's return type) and budget (OBJ_PROP, Z_*_P). */
#include "shim_http_client.h"
#include "ext/standard/php_http.h"
#include "zend_smart_str.h"

zval *swr_zend_property_slot(zend_object *object, const char *name, size_t length) {
    zend_property_info *info = zend_hash_str_find_ptr(&object->ce->properties_info, name, length);
    if (UNEXPECTED(info == NULL || (info->flags & ZEND_ACC_STATIC))) {
        return NULL;
    }
    zval *property = OBJ_PROP(object, info->offset);
    if (UNEXPECTED(property == &EG(uninitialized_zval))) {
        ZVAL_NULL(property);
    }
    return property;
}

zval *swr_zend_property_not_null(zend_object *object, const char *name, size_t length) {
    zval rv;
    zval *property = zend_read_property(object->ce, object, name, length, 0, &rv);
    uint8_t type = Z_TYPE_P(property);
    return (type == IS_NULL || UNEXPECTED(type == IS_UNDEF)) ? NULL : property;
}

void swr_zend_array_add_or_merge(zval *array, const char *key, size_t length, zval *element) {
    zend_ulong index;
    HashTable *table = Z_ARRVAL_P(array);
    bool numeric = ZEND_HANDLE_NUMERIC_STR(key, length, index);
    zval *added = numeric ? zend_hash_index_add(table, index, element) : zend_hash_str_add(table, key, length, element);
    if (added) {
        return;
    }
    zval *current = numeric ? zend_hash_index_find(table, index) : zend_hash_str_find(table, key, length);
    if (Z_TYPE_P(current) == IS_ARRAY) {
        SEPARATE_ARRAY(current);
        add_next_index_zval(current, element);
        return;
    }
    zval list;
    array_init_size(&list, 2);
    Z_ADDREF_P(current);
    add_next_index_zval(&list, current);
    add_next_index_zval(&list, element);
    if (numeric) {
        zend_hash_index_update(table, index, &list);
    } else {
        zend_hash_str_update(table, key, length, &list);
    }
}

bool swr_zval_is_true(const zval *value) {
    return zend_is_true(value);
}

size_t swr_zend_url_decode(char *data, size_t length) {
    return php_url_decode(data, length);
}

zend_string *swr_zend_http_build_query(zval *data) {
    smart_str query = {0};
    if (!HASH_OF(data)) {
        return NULL;
    }
#if PHP_VERSION_ID < 80300
    php_url_encode_hash_ex(HASH_OF(data), &query, NULL, 0, NULL, 0, NULL, 0, NULL, NULL, (int) PHP_QUERY_RFC1738);
#else
    php_url_encode_hash_ex(HASH_OF(data), &query, NULL, 0, NULL, NULL, NULL, (int) PHP_QUERY_RFC1738);
#endif
    if (!query.s) {
        return NULL;
    }
    smart_str_0(&query);
    return query.s;
}

void swr_zend_separate_array(zval *array) {
    SEPARATE_ARRAY(array);
}

bool swr_zend_instanceof(const zend_class_entry *ce, const zend_class_entry *parent) {
    return instanceof_function(ce, parent);
}
