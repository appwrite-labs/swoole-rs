/* Stays C (docs/zend.md, What stays C): globals (EG(uninitialized_zval)), allocator
 * (array_init, smart_str), version (php_url_encode_hash_ex's
 * signature before 8.3, zend_is_true's return type) and budget (OBJ_PROP, Z_*_P). */
#include "shim_http_client.h"
#include "ext/standard/php_http.h"
#include "zend_smart_str.h"


void swr_lent_array_add_or_merge(zval *array, struct iovec name, zval *element) {
    /* ZEND_HANDLE_NUMERIC_STR reads a first byte even of an empty key. */
    const char *key = name.iov_len ? name.iov_base : "";
    size_t length = name.iov_len;
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

bool swr_zval_is_true(zval *value) {
    return zend_is_true(value);
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
    if (Z_TYPE_P(array) == IS_ARRAY) {
        SEPARATE_ARRAY(array);
    }
}

bool swr_zend_class_declares(const zend_class_entry *ce, zend_string *member) {
    return zend_hash_find(&ce->properties_info, member) != NULL;
}

zend_class_entry *swr_zend_class_root(zend_class_entry *ce) {
    /* A parent still held by name (no ZEND_ACC_RESOLVED_PARENT) ends the walk. */
    while ((ce->ce_flags & ZEND_ACC_RESOLVED_PARENT) && ce->parent) {
        ce = ce->parent;
    }
    return ce;
}

bool swr_zend_instanceof(const zend_class_entry *ce, const zend_class_entry *parent) {
    return instanceof_function(ce, parent);
}
