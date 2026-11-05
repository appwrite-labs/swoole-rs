#include "swoole_rs_native_effect.h"
extern int test_invoke(void (*)(void *), void *);
extern void test_resume();
static const swoole_rs::NativeEffectProvider provider{test_invoke, test_resume};
void register_test_provider() { swoole_rs::register_native_effect_provider(&provider); }
const swoole_rs::NativeEffectProvider *test_provider() { return &provider; }
