/* SPDX-License-Identifier: Apache-2.0 */
#include "php.h"
#include "zend_sort.h"

struct item {
    zend_long key;
    size_t position;
};

static int greater(const void *left, const void *right) {
    return ((const struct item *) left)->key > ((const struct item *) right)->key;
}

static int cyclic(const void *left, const void *right) {
    return (((const struct item *) left)->key + 3 - ((const struct item *) right)->key % 3) % 3 == 1;
}

static void exchange(void *left, void *right) {
    struct item tmp = *(struct item *) left;
    *(struct item *) left = *(struct item *) right;
    *(struct item *) right = tmp;
}

ZEND_BEGIN_ARG_INFO_EX(sort_args, 0, 0, 2)
    ZEND_ARG_TYPE_INFO(0, keys, IS_ARRAY, 0)
    ZEND_ARG_TYPE_INFO(0, cyclic, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

PHP_FUNCTION(reference_sort) {
    HashTable *keys;
    bool use_cycle;
    ZEND_PARSE_PARAMETERS_START(2, 2)
        Z_PARAM_ARRAY_HT(keys)
        Z_PARAM_BOOL(use_cycle)
    ZEND_PARSE_PARAMETERS_END();

    size_t n = zend_hash_num_elements(keys);
    struct item *items = safe_emalloc(n, sizeof(*items), 0);
    size_t i = 0;
    zval *key;
    ZEND_HASH_FOREACH_VAL(keys, key) {
        items[i] = (struct item) {zval_get_long(key), i};
        ++i;
    } ZEND_HASH_FOREACH_END();
    zend_sort(items, n, sizeof(*items), use_cycle ? cyclic : greater, exchange);
    array_init(return_value);
    for (i = 0; i < n; ++i) {
        add_next_index_long(return_value, items[i].position);
    }
    efree(items);
}

static const zend_function_entry functions[] = {
    PHP_FE(reference_sort, sort_args)
    PHP_FE_END
};

static zend_module_entry reference_module_entry = {
    STANDARD_MODULE_HEADER, "sort_reference", functions, NULL, NULL, NULL, NULL, NULL,
    "1", STANDARD_MODULE_PROPERTIES
};
ZEND_GET_MODULE(reference)
