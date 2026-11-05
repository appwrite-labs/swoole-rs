#include "swoole_server.h"
#include "swoole_api.h"
#include "swoole_rs_native_effect.h"
#include <cassert>
#include <csetjmp>
#include <memory>
#include <stdexcept>
#include <cstdio>

extern void register_test_provider();
extern const swoole_rs::NativeEffectProvider *test_provider();
static std::jmp_buf boundary;
static bool armed, token_destroyed;
static int requested, invoked, stopped_at;
static bool external_destroyed;
struct ExternalToken { ~ExternalToken() { external_destroyed = true; } };
struct ExternalMarker { std::shared_ptr<ExternalToken> token = std::make_shared<ExternalToken>(); };
static std::exception_ptr external_exception;
struct Token { ~Token() { token_destroyed = true; } };
struct Marker : std::runtime_error {
    std::shared_ptr<Token> token;
    Marker() : std::runtime_error("native custody witness"), token(std::make_shared<Token>()) {}
};
static void callback(void *) {
    if (!armed) return;
    token_destroyed = false;
    stopped_at = invoked;
    throw Marker();
}
int test_invoke(void (*body)(void *), void *context) {
    ++invoked;
    body(context);
    if (!armed || stopped_at != invoked) return 0;
    if (requested == 3) {
        // The native OS entry owns this separate payload. Server must relay the
        // sticky outcome without rethrowing it through an outer Rust frame.
        external_destroyed = false;
        try { throw ExternalMarker(); }
        catch (...) { external_exception = std::current_exception(); }
    }
    return requested;
}
void test_resume() {
    assert(armed && requested > 0);
    assert(token_destroyed && "native Invocation exception_ptr owner must be destroyed before nonlocal resume");
    assert(invoked == stopped_at && "Rust must stop all effects after a fault");
    if (requested == 3) assert(external_exception && !external_destroyed);
    std::longjmp(boundary, requested);
}
static swoole::Server *server() {
    auto *value = new swoole::Server(swoole::Server::MODE_BASE);
    value->worker_num = 1;
    assert(value->add_port(SW_SOCK_TCP, "127.0.0.1", 0));
    assert(value->create() == SW_OK);
    return value;
}
static void ordinary(bool provider) {
    auto *value = server();
    armed = true;
    requested = 0;
    stopped_at = -1;
    bool caught = false;
    try { value->destroy(); }
    catch (const Marker &) { caught = true; }
    assert(caught && token_destroyed);
    if (provider) assert(stopped_at > 0 && invoked == stopped_at);
    armed = false;
    value->destroy();
    delete value;
}
int main() {
    assert(!swoole_rs::native_effect_provider());
    swoole_init();
    swoole_add_hook(SW_GLOBAL_HOOK_AFTER_SERVER_SHUTDOWN, callback, 0);
    ordinary(false);
    register_test_provider();
    assert(swoole_rs::native_effect_provider() == test_provider());
    ordinary(true);
    for (int kind : {1, 2, 3}) {
        auto *value = server();
        requested = kind;
        armed = true;
        stopped_at = -1;
        int resumed = setjmp(boundary);
        if (!resumed) { value->destroy(); assert(false); }
        assert(resumed == kind && token_destroyed && invoked == stopped_at);
        if (kind == 3) {
            bool caught = false;
            try { std::rethrow_exception(external_exception); }
            catch (const ExternalMarker &) { caught = true; }
            assert(caught && !external_destroyed);
            external_exception = nullptr;
            assert(external_destroyed);
        }
        armed = false;
        value->destroy();
        delete value;
    }
    swoole_clean();
    std::puts("production Server native effect custody: PASS");
}
