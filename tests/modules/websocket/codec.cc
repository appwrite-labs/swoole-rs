// Differential against unchanged pinned codec function bodies, included by codec.py.
#include "swoole_rs_websocket_codec.h"
#include <algorithm>
#include <array>
#include <cassert>
#include <climits>
#include <cstdio>
#include <cstring>
#include <vector>
#include "oracle.inc"

int main() {
    static_assert(sizeof(swr_ws_decoded) == 24);
    static_assert(sizeof(swr_ws_context) == 24);
    static_assert(sizeof(swr_ws_decision) == 32);
    size_t encoded = 0, decoded = 0, closes = 0, decisions = 0, php_policy = 0;
    for (unsigned opcode = 0; opcode < 256; ++opcode) {
        for (unsigned flags = 0; flags < 256; ++flags) {
            for (size_t length : {size_t(0), size_t(1), size_t(65536), SIZE_MAX}) {
                assert(swr_v1_ws_pack_policy(opcode, flags, length) == reference::pack_policy(opcode, flags, length));
                ++php_policy;
            }
            assert(swr_v1_ws_message_policy(opcode, flags) == reference::message_policy(opcode, flags));
            ++php_policy;
        }
    }
    for (size_t length : {size_t(0), size_t(1), size_t(7), size_t(8), size_t(125), size_t(126), size_t(65535), size_t(65536)}) {
        std::vector<uint8_t> payload(length);
        for (size_t i = 0; i < length; ++i) payload[i] = uint8_t(i * 37 + 19);
        for (unsigned flags = 0; flags < 256; ++flags) {
            for (unsigned opcode = 0; opcode < 16; ++opcode) {
                reference::String oracle;
                const bool accepted = reference::encode(&oracle, reinterpret_cast<const char *>(payload.data()), length, opcode, flags);
                const size_t size = swr_v1_ws_encoded_size(length, flags);
                std::vector<uint8_t> wire(size + 2, 0xa5);
                const size_t written = swr_v1_ws_encode(payload.data(), length, opcode, flags, wire.data() + 1, size);
                // Close/ping/pong must be final and at most 125 bytes; both sides reject before writing.
                if (swr_v1_ws_control_invalid(opcode, flags, length)) {
                    assert(!accepted && oracle.length == 0 && written == 0);
                    assert(std::all_of(wire.begin(), wire.end(), [](uint8_t b) { return b == 0xa5; }));
                    ++encoded;
                    continue;
                }
                assert(written == size);
                assert(wire.front() == 0xa5 && wire.back() == 0xa5);
                assert(oracle.length == size && std::memcmp(oracle.str, wire.data() + 1, size) == 0);
                assert(accepted == !((flags & 32) && (flags & 64)));
                ++encoded;
                // Every header prefix and incomplete/complete body, plus trailing bytes.
                if (flags & 64) continue;
                const size_t header = size - length;
                std::vector<size_t> prefixes;
                for (size_t prefix = 2; prefix <= header; ++prefix) prefixes.push_back(prefix);
                if (length) prefixes.push_back(size - 1);
                prefixes.push_back(size);
                prefixes.push_back(size + 1);
                for (size_t prefix : prefixes) {
                    // Allocate exactly the advertised prefix so ASan can detect
                    // reads past an incomplete header/body, not only writes.
                    std::vector<char> native(wire.begin() + 1, wire.begin() + 1 + prefix);
                    std::vector<uint8_t> rust(native.begin(), native.end());
                    reference::Frame expected{};
                    swr_ws_decoded actual{};
                    const bool success = reference::decode(&expected, native.data(), prefix);
                    const auto total = swr_v1_ws_decode(rust.data(), prefix, &actual);
                    assert(success == (total > 0 && size_t(total) <= prefix));
                    assert(actual.opcode == expected.header.OPCODE);
                    assert(actual.flags == expected.get_flags());
                    assert(actual.length_code == expected.header.LENGTH);
                    assert(std::equal(native.begin(), native.end(), rust.begin(), [](char a, uint8_t b) { return uint8_t(a) == b; }));
                    if (success) {
                        assert(actual.header_length == expected.header_length);
                        assert(actual.payload_length == expected.payload_length);
                        if (length && (flags & 32)) assert(std::memcmp(actual.mask_key, expected.mask_key, 4) == 0);
                    }
                    ++decoded;
                }
            }
        }
    }
    for (int code : {INT_MIN, -1, 0, 1000, 65535, 65536, INT_MAX}) {
        for (size_t length : {size_t(0), size_t(1), size_t(123), size_t(124), size_t(125), size_t(126)}) {
            std::array<uint8_t, 126> reason{};
            for (size_t i = 0; i < reason.size(); ++i) reason[i] = uint8_t(i + 128);
            for (unsigned flags = 0; flags < 256; ++flags) {
                reference::String oracle;
                const bool accepted = reference::pack_close_frame(&oracle, code, reinterpret_cast<const char *>(reason.data()), length, flags);
                std::array<uint8_t, 144> wire;
                wire.fill(0xa5);
                const size_t size = swr_v1_ws_encode_close(code, reason.data(), length, flags, wire.data() + 1, wire.size() - 2);
                // Code plus reason is a control payload: at most 125 bytes.
                if (length > 123) { assert(size == 0 && oracle.length == 0 && !accepted); }
                else {
                    assert(size == oracle.length && std::memcmp(wire.data() + 1, oracle.str, size) == 0);
                    assert(accepted == !((flags & 32) && (flags & 64)));
                }
                assert(wire[0] == 0xa5 && wire[size + 1] == 0xa5);
                ++closes;
            }
        }
    }
    // Decoder reads all RSV bits, including combinations the encoder clears.
    for (unsigned first = 0; first < 256; ++first) {
        for (uint8_t second : {uint8_t(0), uint8_t(128)}) {
            std::array<char, 6> native{char(first), char(second), 1, 2, 3, 4};
            auto rust = native;
            reference::Frame expected{};
            swr_ws_decoded actual{};
            const size_t length = second ? 6 : 2;
            // Compressed (RSV1) close/ping/pong frames are rejected before unmasking.
            const unsigned opcode = first & 15;
            const bool invalid = (first & 64) && opcode >= 8 && opcode <= 10;
            assert(reference::decode(&expected, native.data(), length) == !invalid);
            assert(swr_v1_ws_decode(reinterpret_cast<uint8_t *>(rust.data()), length, &actual) ==
                   (invalid ? SWR_WS_INVALID_CONTROL : int64_t(length)));
            assert(native == rust);
            assert(actual.flags == expected.get_flags() && actual.opcode == expected.header.OPCODE);
            ++decoded;
        }
    }
    // Pin observed policy: wire-length control bounds, permissive FIN/RSV,
    // orphan/overlapping fragments, FIN message during active fragmentation,
    // exact aggregate limit and preserved initial fragment opcode/flags.
    auto check = [&](uint8_t opcode, uint8_t flags, size_t wire, size_t payload,
                     swr_ws_context context, uint32_t action, uint32_t error) {
        swr_ws_decoded frame{};
        frame.opcode = opcode; frame.flags = flags; frame.payload_length = payload;
        frame.header_length = (flags & 32) ? 6 : 2;
        swr_ws_decision result{};
        assert(swr_v1_ws_decide(&frame, wire, &context, &result) == 0);
        assert(result.action == action && result.error == error);
        if (action == SWR_WS_FINISH) assert(result.ext_flags == (context.fragment_flags | 1));
        ++decisions;
    };
    for (uint8_t flags : {uint8_t(0), uint8_t(1), uint8_t(32), uint8_t(33), uint8_t(61)}) {
        const size_t header = (flags & 32) ? 6 : 2;
        check(0, flags, header + 1, 1, {}, SWR_WS_REJECT, SWR_WS_ORPHAN);
        swr_ws_context active{65535, 65536, 0x021c, 1, 0};
        check(0, flags, header + 1, 1, active, (flags & 1) ? SWR_WS_FINISH : SWR_WS_APPEND, 0);
        check(0, flags, header + 2, 2, active, SWR_WS_REJECT, SWR_WS_TOO_BIG);
        active.fragment_length = SIZE_MAX;
        check(0, flags, header + 1, 1, active, SWR_WS_REJECT, SWR_WS_TOO_BIG);
        for (uint8_t opcode : {uint8_t(1), uint8_t(2)}) {
            check(opcode, flags, header + 1, 1, {}, (flags & 1) ? SWR_WS_DISPATCH : SWR_WS_START, 0);
            check(opcode, flags, header + 1, 1, active, (flags & 1) ? SWR_WS_DISPATCH : SWR_WS_REJECT,
                  (flags & 1) ? 0 : SWR_WS_OVERLAP);
        }
        for (uint8_t opcode : {uint8_t(9), uint8_t(10)}) {
            check(opcode, flags, 126, 126 - header, active, SWR_WS_CONTROL, 0);
            check(opcode, flags, 127, 127 - header, active, SWR_WS_REJECT, SWR_WS_CONTROL_TOO_BIG);
        }
        check(8, flags, 127, 127 - header, active, SWR_WS_CLIENT_CLOSE, 0);
        check(8, flags, 128, 128 - header, active, SWR_WS_REJECT, SWR_WS_CLOSE_TOO_BIG);
        active.closing = 1;
        check(8, flags, header, 0, active, SWR_WS_SERVER_CLOSE, 0);
        check(15, flags, header, 0, active, SWR_WS_UNKNOWN, 0);
    }
    std::array<uint8_t, 16> untouched{};
    untouched.fill(0xa5);
    const auto original = untouched;
    for (size_t capacity = 0; capacity < 7; ++capacity) {
        assert(swr_v1_ws_encode(reinterpret_cast<const uint8_t *>("x"), 1, 1, 33, untouched.data(), capacity) == 0);
        assert(untouched == original);
    }
    assert(swr_v1_ws_encode(nullptr, 1, 1, 1, untouched.data(), untouched.size()) == 0 && untouched == original);
    assert(swr_v1_ws_encode(untouched.data(), 1, 1, 1, nullptr, 16) == 0);
    assert(swr_v1_ws_encode(untouched.data(), 1, 1, 1, untouched.data(), SIZE_MAX) == 0 && untouched == original);
    assert(swr_v1_ws_encode_close(1000, nullptr, 1, 0, untouched.data(), untouched.size()) == 0 && untouched == original);
    assert(swr_v1_ws_encoded_size(SIZE_MAX, 0) == 0);
    swr_ws_decoded empty{};
    assert(swr_v1_ws_decode(nullptr, 0, &empty) == 0);
    assert(swr_v1_ws_decode(nullptr, 1, &empty) == -1);
    assert(swr_v1_ws_decode(untouched.data(), 2, nullptr) == -1 && untouched == original);
    assert(swr_v1_ws_decode(untouched.data(), SIZE_MAX, &empty) == -1 && untouched == original);
    swr_ws_context context{};
    swr_ws_decision decision{};
    assert(swr_v1_ws_decide(nullptr, 2, &context, &decision) == -1);
    assert(swr_v1_ws_decide(&empty, 2, nullptr, &decision) == -1);
    assert(swr_v1_ws_decide(&empty, 2, &context, nullptr) == -1);
    empty.flags = 32; empty.header_length = 2;
    assert(swr_v1_ws_decide(&empty, 2, &context, &decision) == -1);
    // Upstream accepts a non-minimal extended ping header but dispatches from
    // offset two, including the extended-length bytes in application data.
    swr_ws_decoded extended{};
    extended.opcode = 9; extended.header_length = 4; extended.payload_length = 1;
    assert(swr_v1_ws_decide(&extended, 5, &context, &decision) == 0);
    assert(decision.action == SWR_WS_CONTROL && decision.offset == 2 && decision.length == 3);
    ++decisions;
    std::printf("{\"encoded\":%zu,\"decoded_prefixes\":%zu,\"close_cases\":%zu,\"policy_cases\":%zu,\"php_policy_cases\":%zu,\"errors\":0}\n", encoded, decoded, closes, decisions, php_policy);
}
