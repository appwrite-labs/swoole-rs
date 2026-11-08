#include "test_coroutine.h"
#include "swoole_rs_test_allocation.h"
#include <cstring>
using swoole::coroutine::Socket;
using swoole::coroutine::ProtocolSwitch;

TEST(rust_socket_scopes, protocol_snapshot_and_copy_restore) {
    swoole::test::coroutine::run([](void *) {
        Socket socket(SW_SOCK_TCP);
        socket.open_eof_check = true;
        socket.open_length_check = false;
        std::memset(&socket.protocol, 0, sizeof(socket.protocol));
        socket.protocol.package_max_length = 117;
        socket.protocol.package_eof[0] = '\0';
        socket.protocol.package_eof[1] = 'z';
        const auto original = socket.protocol;
        {
            ProtocolSwitch scope(&socket);
            auto copy = scope;
            socket.open_eof_check = false;
            socket.open_length_check = true;
            std::memset(&socket.protocol, 1, sizeof(socket.protocol));
        }
        ASSERT_TRUE(socket.open_eof_check);
        ASSERT_FALSE(socket.open_length_check);
        ASSERT_EQ(std::memcmp(&original, &socket.protocol, sizeof(original)), 0);
        {
            ProtocolSwitch first(&socket);
            socket.protocol.package_max_length = 222;
            {
                ProtocolSwitch nested(&socket);
                socket.protocol.package_max_length = 333;
            }
            ASSERT_EQ(socket.protocol.package_max_length, 222u);
        }
        ASSERT_EQ(socket.protocol.package_max_length, 117u);
    });
}

TEST(rust_socket_scopes, error_text_publication_and_cache_exception) {
    swoole::test::coroutine::run([](void *) {
        Socket socket(SW_SOCK_TCP);
        swoole_set_last_error(0);
        errno = EINVAL;
        socket.set_err();
        ASSERT_EQ(socket.errCode, EINVAL);
        ASSERT_EQ(swoole_get_last_error(), 0);
        ASSERT_EQ(errno, EINVAL);
        swoole_set_last_error(EACCES);
        socket.set_err();
        ASSERT_EQ(socket.errCode, EACCES);
        ASSERT_EQ(errno, EINVAL);
        const char *borrowed = "borrowed";
        socket.set_err(EIO, borrowed);
        ASSERT_EQ(socket.errMsg, borrowed);
        ASSERT_EQ(errno, EIO);
        ASSERT_EQ(swoole_get_last_error(), EIO);
        const std::string first(100, 'a');
        const std::string second(600, 'b');
        socket.set_err(ERANGE, first);
        const std::string &cache = socket.errString;
        const std::string *identity = &cache;
        const char *message = socket.errMsg;
        allocation::begin(1);
        bool caught = false;
        try { socket.set_err(ENOSPC, second); }
        catch (const std::bad_alloc &) { caught = true; }
        allocation::end();
        ASSERT_TRUE(caught);
        ASSERT_EQ(socket.errCode, ENOSPC);
        ASSERT_EQ(errno, ENOSPC);
        ASSERT_EQ(swoole_get_last_error(), ENOSPC);
        ASSERT_EQ(socket.errMsg, message);
        ASSERT_EQ(cache, first);
        ASSERT_EQ(&static_cast<const std::string &>(socket.errString), identity);
        socket.set_err(0, std::string("a\0b", 3));
        ASSERT_EQ(static_cast<const std::string &>(socket.errString), std::string("a\0b", 3));
        ASSERT_EQ(socket.errMsg[2], 'b');
    });
}
