#include "test_coroutine.h"
#include <new>
using swoole::Coroutine;

struct ThrowingCancelCopy {
    bool *fail;
    explicit ThrowingCancelCopy(bool *value) : fail(value) {}
    ThrowingCancelCopy(const ThrowingCancelCopy &other) : fail(other.fail) {
        if (*fail) { throw std::bad_alloc(); }
    }
    bool operator()(Coroutine *co) const {
        co->resume();
        return true;
    }
};

TEST(rust_coroutine_lifetime, cancellation_copy_failure_preserves_wait) {
    bool fail = false;
    bool resumed_canceled = false;
    swoole::test::coroutine::run([&](void *) {
        long cid = Coroutine::create([&](void *) {
            Coroutine::CancelFunc cancel = ThrowingCancelCopy(&fail);
            Coroutine *co = Coroutine::get_current();
            co->yield(&cancel);
            resumed_canceled = co->is_canceled();
        });
        Coroutine *co = Coroutine::get_by_cid(cid);
        ASSERT_TRUE(co->is_suspending());
        fail = true;
        EXPECT_THROW(co->cancel(), std::bad_alloc);
        ASSERT_TRUE(co->is_suspending());
        ASSERT_FALSE(co->is_canceled());
        fail = false;
        ASSERT_TRUE(co->cancel());
        // cancel's callback has resumed and deleted co; never read it again.
        ASSERT_EQ(Coroutine::get_by_cid(cid), nullptr);
        ASSERT_TRUE(resumed_canceled);
    });
    ASSERT_EQ(Coroutine::count(), 0);
}
