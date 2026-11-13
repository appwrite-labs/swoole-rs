// The pinned Port event catalog (ext-src/swoole_server_port.cc's server_port_event_map
// over php_swoole_server.h's callback enum), swoole.h's swoole_strcaseeq and the pinned
// getCallback body behind C entry points: build.rs copies them unchanged. The lookup is
// the retired tests/modules/ports/gate.cc's and the getter runs in the retired
// callback_property.cc's model of the Zend calls it makes, which records their order.
// The candidate is crates/ports' catalog (tests/modules/ports/catalog.rs).
#include <cassert>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>
#include <strings.h>
#include <unordered_map>
#include <vector>

namespace upstream_ports {
#include "ports_catalog.inc"

using Entry = std::pair<const std::string, ServerPortEvent>;
// The catalog in its iteration order, which numbers the model's properties.
static const std::vector<const Entry *> &entries() {
    static const std::vector<const Entry *> list = [] {
        std::vector<const Entry *> all;
        for (const auto &entry : server_port_event_map) {
            all.push_back(&entry);
        }
        return all;
    }();
    return list;
}

// Each Zend call the getter makes, in order.
static std::string trace;
static unsigned lowercase_calls;
struct zval {
    std::string bytes;
    bool is_null = false;
    unsigned identity = 0;
};
struct zend_string {
    std::string bytes;
};
namespace zend {
class String {
    zend_string storage;

  public:
    explicit String(zval *input) {
        trace += "convert;";
        storage.bytes = input->bytes;
    }
    String(zend_string *input, bool) {
        storage.bytes = std::move(input->bytes);
        delete input;
    }
    zend_string *get() {
        return &storage;
    }
    std::string to_std_string() const {
        return storage.bytes;
    }
};
}  // namespace zend
// Zend's zend_tolower_map folds ASCII letters only.
static zend_string *zend_string_tolower(zend_string *input) {
    lowercase_calls++;
    auto *result = new zend_string{input->bytes};
    for (char &c : result->bytes) {
        c = c >= 'A' && c <= 'Z' ? static_cast<char>(c + 32) : c;
    }
    return result;
}
static std::unordered_map<std::string, zval> properties;
static int object, entry;
static int *swoole_server_port_ce = &entry;
static zval null_value{"", true, 0};
static zval *zend_read_property(int *ce, int *obj, const char *name, size_t size, bool silent, zval *) {
    assert(ce == swoole_server_port_ce && obj == &object && silent);
    trace += "read:" + std::string(name, size) + ';';
    auto i = properties.find(std::string(name, size));
    return i == properties.end() ? &null_value : &i->second;
}
#define PHP_METHOD(c, m) zval *getCallback(zval *input)
#define ZEND_PARSE_PARAMETERS_START(a, b)
#define Z_PARAM_ZVAL(name) name = input;
#define ZEND_PARSE_PARAMETERS_END_EX(fallback)
#define ZEND_THIS (&object)
#define SW_Z8_OBJ_P(p) (p)
#define ZVAL_IS_NULL(p) ((p)->is_null)
#define RETURN_FALSE return nullptr
#define RETURN_NULL()                                                                                                  \
    do {                                                                                                               \
        trace += "null;";                                                                                              \
        return nullptr;                                                                                                \
    } while (0)
#define RETURN_ZVAL(p, copy, dtor)                                                                                     \
    do {                                                                                                               \
        static_assert(copy == 1 && dtor == 0, "return flags");                                                         \
        trace += "copy:" + std::to_string((p)->identity) + ';';                                                        \
        return (p);                                                                                                    \
    } while (0)
#include "ports_get_callback.inc"
}  // namespace upstream_ports

extern "C" {
size_t swr_oracle_ports_events(void) {
    return upstream_ports::entries().size();
}
// Entry `index`'s key, callback type and name (its property is "on" and the name).
int swr_oracle_ports_entry(size_t index, const char **key, size_t *key_length, const char **name, size_t *name_length) {
    const auto *entry = upstream_ports::entries().at(index);
    *key = entry->first.data();
    *key_length = entry->first.size();
    *name = entry->second.name.data();
    *name_length = entry->second.name.size();
    return entry->second.type;
}
// The callback type `name` registers, exactly or as swoole_strcaseeq folds it, and
// the entry's name; -1 for none.
int swr_oracle_ports_event(const char *name, size_t length, bool folded, const char **found, size_t *found_length) {
    for (const auto &entry : upstream_ports::server_port_event_map) {
        if (folded ? upstream_ports::swoole_strcaseeq(name, length, entry.first.data(), entry.first.size())
                   : std::string(name, length) == entry.first) {
            *found = entry.second.name.data();
            *found_length = entry.second.name.size();
            return entry.second.type;
        }
    }
    return -1;
}
// The model's object: each event's property holds a callback, numbered from 1 in
// entry order, or null.
void swr_oracle_ports_properties(bool null_properties) {
    unsigned id = 1;
    upstream_ports::properties.clear();
    for (const auto *entry : upstream_ports::entries()) {
        upstream_ports::properties.emplace("on" + entry->second.name,
                                           upstream_ports::zval{"callback", null_properties, id++});
    }
}
// getCallback(name): the number of the property it returned (0 for null), its Zend
// calls in order, and how many times it lowercased.
unsigned swr_oracle_ports_get_callback(
    const char *name, size_t length, const char **calls, size_t *calls_length, unsigned *lowercased) {
    upstream_ports::zval input{std::string(name, length), false, 0};
    upstream_ports::trace.clear();
    upstream_ports::lowercase_calls = 0;
    const auto *returned = upstream_ports::getCallback(&input);
    *calls = upstream_ports::trace.data();
    *calls_length = upstream_ports::trace.size();
    *lowercased = upstream_ports::lowercase_calls;
    return returned ? returned->identity : 0;
}
}
