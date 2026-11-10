/* Stays C (docs/zend.md, What stays C): globals (EG(current_execute_data),
 * EG(exception)), version (the allocator hooks' debug signature from 8.4,
 * number_format before 8.3), bailout (ZEND_WRITE) and allocator (smart_str). */
#include "shim.h"
#include "shim_tracer.h"
#include "ext/standard/php_math.h"
#include "ext/json/php_json.h"
#include "zend_smart_str.h"

/* PHP 8.4 passes the custom handlers the caller's file and line in debug builds. */
#if PHP_VERSION_ID < 80400
#define MM_LINE_DC
#define MM_LINE_ORIG_DC
#define MM_LINE_CC
#define MM_LINE_ORIG_RELAY_CC
#else
#define MM_LINE_DC ZEND_FILE_LINE_DC
#define MM_LINE_ORIG_DC ZEND_FILE_LINE_ORIG_DC
#define MM_LINE_CC ZEND_FILE_LINE_RELAY_CC
#define MM_LINE_ORIG_RELAY_CC ZEND_FILE_LINE_ORIG_RELAY_CC
#endif

static const swr_zend_mm_observer *observer;
static void *(*ori_malloc)(size_t MM_LINE_DC MM_LINE_ORIG_DC);
static void (*ori_free)(void *MM_LINE_DC MM_LINE_ORIG_DC);
static void *(*ori_realloc)(void *, size_t MM_LINE_DC MM_LINE_ORIG_DC);

static void *observed_malloc(size_t size MM_LINE_DC MM_LINE_ORIG_DC) {
    void *ptr = ori_malloc ? ori_malloc(size MM_LINE_CC MM_LINE_ORIG_RELAY_CC) : zend_mm_alloc(zend_mm_get_heap(), size);
    const swr_zend_mm_observer *current = observer;
    swr_zend_mm_unobserve();
    current->alloc(ptr, size);
    swr_zend_mm_observe(current);
    return ptr;
}

static void *observed_realloc(void *ptr, size_t size MM_LINE_DC MM_LINE_ORIG_DC) {
    void *new_ptr = ori_realloc ? ori_realloc(ptr, size MM_LINE_CC MM_LINE_ORIG_RELAY_CC)
                                : zend_mm_realloc(zend_mm_get_heap(), ptr, size);
    const swr_zend_mm_observer *current = observer;
    swr_zend_mm_unobserve();
    current->realloc(ptr, new_ptr, size);
    swr_zend_mm_observe(current);
    return new_ptr;
}

static void observed_free(void *ptr MM_LINE_DC MM_LINE_ORIG_DC) {
    if (ori_free) {
        ori_free(ptr MM_LINE_CC MM_LINE_ORIG_RELAY_CC);
    } else {
        zend_mm_free(zend_mm_get_heap(), ptr);
    }
    observer->free(ptr);
}

void swr_zend_mm_observe(const swr_zend_mm_observer *next) {
    zend_mm_heap *heap = zend_mm_get_heap();
    observer = next;
    zend_mm_get_custom_handlers(heap, &ori_malloc, &ori_free, &ori_realloc);
    zend_mm_set_custom_handlers(heap, observed_malloc, observed_free, observed_realloc);
}

void swr_zend_mm_unobserve(void) {
    zend_mm_heap *heap = zend_mm_get_heap();
    if (ori_malloc || ori_free || ori_realloc) {
        zend_mm_set_custom_handlers(heap, ori_malloc, ori_free, ori_realloc);
        ori_malloc = NULL;
        ori_free = NULL;
        ori_realloc = NULL;
    } else {
        /* heap->use_custom_heap, the heap's first member. */
        *((int *) heap) = 0;
    }
}

uint32_t swr_zend_tracer_lineno(void) {
    zend_execute_data *ex = EG(current_execute_data);
    while (ex && (!ex->func || !ZEND_USER_CODE(ex->func->type))) {
        ex = ex->prev_execute_data;
    }
    if (!ex || !ex->opline) {
        return 0;
    }
    if (EG(exception) && ex->opline->opcode == ZEND_HANDLE_EXCEPTION && ex->opline->lineno == 0 &&
        EG(opline_before_exception)) {
        return EG(opline_before_exception)->lineno;
    }
    return ex->opline->lineno;
}

int swr_zend_try_write(const char *text, size_t length) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        ZEND_WRITE(text, length);
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    return bailed;
}

zend_string *swr_zend_number_format(zend_long number) {
#if PHP_VERSION_ID >= 80300
    return _php_math_number_format_long(number, 0, ".", 1, ",", 1);
#else
    return _php_math_number_format((double) number, 0, '.', ',');
#endif
}

zend_string *swr_zend_json_encode(zval *value) {
    smart_str buf = {0};
    if (php_json_encode(&buf, value, 0) == FAILURE) {
        smart_str_free(&buf);
        return NULL;
    }
    return smart_str_extract(&buf);
}

int swr_zend_try_json_encode(zval *value, zend_string **out) {
    smart_str buf = {0};
    volatile int bailed = 0;
    *out = NULL;
    SWR_ZEND_TRY {
        if (php_json_encode(&buf, value, 0) != FAILURE && buf.s) {
            *out = buf.s;
        }
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    if (!*out) {
        smart_str_free(&buf);
    }
    return bailed;
}

/* The scanner reads one byte past `length` and takes only a NUL there as the
 * end, as upstream's std::string::c_str() guarantees; a reply buffer has none. */
void swr_zend_json_decode(const char *text, size_t length, zval *out) {
    zend_string *copy = zend_string_init(text, length, 0);
    if (php_json_decode(out, ZSTR_VAL(copy), length, 1, 0) == FAILURE) {
        ZVAL_FALSE(out);
    }
    zend_string_release(copy);
}
