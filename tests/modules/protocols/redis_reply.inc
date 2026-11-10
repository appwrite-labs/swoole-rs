// redis::format and redis::parse over Rust's RESP codec (crates/protocols-ffi/src/redis.rs).
// The build has no callers of them, so they are not built; protocols-native (native.cc)
// includes this inside its `redis` namespace, over its own `String`.
static void append_reply(String *buffer, uint8_t kind, const char *data, size_t length, int64_t integer = 0) {
    const auto *input = reinterpret_cast<const uint8_t *>(data);
    const size_t size = swr_v1_resp_format(kind, input, length, integer, nullptr, 0);
    if (!size || size > SIZE_MAX - buffer->length) return;
    if (buffer->length + size > buffer->size) buffer->reserve(buffer->length + size);
    if (swr_v1_resp_format(kind, input, length, integer,
                           reinterpret_cast<uint8_t *>(buffer->str + buffer->length), size) == size) {
        buffer->length += size;
    }
}

void format_nil(String *buffer) { append_reply(buffer, 7, nullptr, 0); }
void format(String *buffer, ReplyType type, const std::string &value) {
    append_reply(buffer, type, value.data(), value.size());
}
void format(String *buffer, ReplyType, long value) { append_reply(buffer, 8, nullptr, 0, value); }

std::vector<std::string> parse(const char *data, size_t length) {
    const auto *input = reinterpret_cast<const uint8_t *>(data);
    uint8_t range = 0;
    swr_resp_span local[16];
    auto *spans = local;
    const auto count = swr_v1_resp_scan(input, length, local, 16, &range);
    if (range) errno = ERANGE;
    if (count < 0) return {};
    std::vector<swr_resp_span> overflow;
    if (count > 16) {
        overflow.resize(count);
        spans = overflow.data();
        if (swr_v1_resp_scan(input, length, spans, count, &range) != count) return {};
    }
    std::vector<std::string> result;
    result.reserve(count);
    for (int64_t i = 0; i < count; ++i) {
        const auto &span = spans[i];
        if (span.kind == 1) result.push_back(std::to_string(span.integer));
        else result.emplace_back(data + span.offset, span.length);
    }
    return result;
}
