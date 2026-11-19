/* SPDX-License-Identifier: Apache-2.0 */
#include "php.h"
#include <inttypes.h>

ZEND_BEGIN_ARG_INFO_EX(plain, 0, 1, 2)
    ZEND_ARG_VARIADIC_INFO(1, arg)
    ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, arg, IS_LONG, 1, "null")
    ZEND_ARG_TYPE_INFO(0, arg, IS_MIXED, 0)
    ZEND_ARG_VARIADIC_TYPE_INFO(0, arg, IS_ITERABLE, 0)
    ZEND_ARG_TYPE_MASK(0, arg, MAY_BE_LONG | MAY_BE_FALSE, NULL)
    ZEND_ARG_VARIADIC_OBJ_INFO(1, arg, Example, 1)
    ZEND_ARG_OBJ_TYPE_MASK(1, arg, Example, MAY_BE_FALSE, "null")
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(typed, 0, 1, _IS_BOOL, 0)
ZEND_END_ARG_INFO()
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_MASK_EX(masked, 0, 0, MAY_BE_LONG | MAY_BE_NULL)
ZEND_END_ARG_INFO()
ZEND_BEGIN_ARG_WITH_RETURN_OBJ_INFO_EX(object, 1, 2, Example, 1)
ZEND_END_ARG_INFO()
ZEND_BEGIN_ARG_WITH_RETURN_OBJ_TYPE_MASK_EX(object_mask, 0, 3, Example, MAY_BE_FALSE)
ZEND_END_ARG_INFO()

static void row(const zend_internal_arg_info *slot, bool result) {
    if (result) {
        printf("%" PRIuPTR, (uintptr_t) slot->name);
    } else {
        printf("%s", slot->name);
    }
    printf(" %u %s %s\n", slot->type.type_mask,
           slot->type.ptr ? (const char *) slot->type.ptr : "-",
           slot->default_value ? slot->default_value : "-");
}

int main(void) {
    for (size_t i = 0; i < sizeof(plain) / sizeof(*plain); ++i) {
        row(&plain[i], i == 0);
    }
    row(typed, true);
    row(masked, true);
    row(object, true);
    row(object_mask, true);
    return 0;
}
