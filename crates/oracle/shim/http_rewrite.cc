// The pinned Server::add_rewrite_rule and apply_rewrite_rules (std::regex) behind C
// entry points: build.rs copies both bodies unchanged from static_handler.cc. The
// handler publishes into strings and counts each publication, as the retired
// tests/modules/http/rewrite_gate.py's oracle did. The candidate is crates/http's
// RewriteRules over its static context (tests/modules/http/rewrite.rs).
#include <algorithm>
#include <cstddef>
#include <memory>
#include <regex>
#include <string>
#include <vector>

namespace upstream_rewrite {
// As upstream include/swoole_util.h.
static inline bool starts_with(const std::string &str, const std::string &prefix) {
    if (prefix.size() > str.size()) {
        return false;
    }
    return std::equal(prefix.begin(), prefix.end(), str.begin());
}
namespace http_server {
struct RewriteRule {
    std::string pattern, replacement;
    bool is_regex = false;
};
struct StaticHandler {
    std::string url, original;
    unsigned publications = 0;
    explicit StaticHandler(const std::string &value) : url(value), original(value) {}
    const std::string &get_request_url() const {
        return url;
    }
    void set_request_url(const std::string &value) {
        ++publications;
        url = value;
    }
};
}  // namespace http_server
struct Server {
    std::shared_ptr<std::vector<http_server::RewriteRule>> rewrite_rules;
    void add_rewrite_rule(const std::string &, const std::string &);
    bool apply_rewrite_rules(http_server::StaticHandler *);
};
#include "http_rewrite.inc"
}  // namespace upstream_rewrite

extern "C" {
struct swr_oracle_rewrite_rule {
    const char *pattern;
    size_t pattern_length;
    const char *replacement;
    size_t replacement_length;
};
typedef void (*swr_oracle_rewrite_text)(void *out, const char *text, size_t length);
// Adds each rule to a new server and applies them to a handler for `url`: the handler's
// URL and then its original URL go to `text`, and its set_request_url calls are counted
// in `publications`. apply_rewrite_rules' result, or -1 for an exception it let through.
int swr_oracle_rewrite(const swr_oracle_rewrite_rule *rules,
                       size_t count,
                       const char *url,
                       size_t length,
                       unsigned *publications,
                       swr_oracle_rewrite_text text,
                       void *out) {
    try {
        upstream_rewrite::Server server;
        for (size_t i = 0; i < count; i++) {
            server.add_rewrite_rule(std::string(rules[i].pattern, rules[i].pattern_length),
                                    std::string(rules[i].replacement, rules[i].replacement_length));
        }
        upstream_rewrite::http_server::StaticHandler handler(std::string(url, length));
        const bool applied = server.apply_rewrite_rules(&handler);
        *publications = handler.publications;
        text(out, handler.url.data(), handler.url.size());
        text(out, handler.original.data(), handler.original.size());
        return applied;
    } catch (...) {
        return -1;
    }
}
// A server without rules returns before it reads its handler, which is null here.
int swr_oracle_rewrite_without_rules() {
    upstream_rewrite::Server server;
    return server.apply_rewrite_rules(nullptr);
}
}
