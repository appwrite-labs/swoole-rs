// SPDX-License-Identifier: Apache-2.0
// What crates/phpclients' resolver.rs needs from C++: NameResolver::Context and
// swoole.h's resolver list, which core code and the coroutine socket share, with
// their std::string, std::list and std::function signatures.
#include "php_swoole_cxx.h"
#include "swoole_rs_phpclients.h"

using swoole::NameResolver;

static NameResolver::Context *context_of(void *context) {
    return static_cast<NameResolver::Context *>(context);
}

static std::string php_lookup(const std::string &name, NameResolver::Context *ctx, void *resolver) {
    std::string result;
    swr_v1_phpclients_name_resolver_lookup(
        static_cast<zval *>(resolver), name.data(), name.size(), ctx, &result, [](void *out, const char *text, size_t length) {
            static_cast<std::string *>(out)->assign(text, length);
        });
    return result;
}

void swoole_name_resolver_php_add(zval *resolver) {
    swoole_name_resolver_add({php_lookup, resolver, NameResolver::TYPE_PHP});
}

zval *swoole_name_resolver_php_remove(bool (*same)(zval *kept, zval *resolver), zval *resolver) {
    zval *removed = nullptr;
    swoole_name_resolver_each([&](const std::list<NameResolver>::iterator &iter) -> swTraverseOperation {
        if (removed) {
            return SW_TRAVERSE_STOP;
        }
        if (iter->type == NameResolver::TYPE_PHP && iter->private_data && same((zval *) iter->private_data, resolver)) {
            removed = (zval *) iter->private_data;
            return SW_TRAVERSE_REMOVE;
        }
        return SW_TRAVERSE_KEEP;
    });
    return removed;
}

void swoole_name_resolver_lookup_run(void *call) {
    auto *c = static_cast<swoole_name_resolver_lookup_call *>(call);
    auto result = swoole_name_resolver_lookup(std::string(c->name, c->length), context_of(c->context));
    c->assign(c->out, result.data(), result.size());
}

void *swoole_name_resolver_context_new(void) {
    return new NameResolver::Context();
}

void swoole_name_resolver_context_free(void *context) {
    delete context_of(context);
}

void swoole_name_resolver_context_reset(void *context, bool with_port) {
    *context_of(context) = {};
    context_of(context)->with_port = with_port;
}

void swoole_name_resolver_context_set(void *context, int64_t type, bool with_port) {
    context_of(context)->with_port = with_port;
    context_of(context)->type = type;
}

zval *swoole_name_resolver_context_get(void *context, bool *with_port) {
    *with_port = context_of(context)->with_port;
    return static_cast<zval *>(context_of(context)->private_data);
}

// The cluster object is the context's until release (or the context's destructor) frees it.
static void free_cluster(NameResolver::Context *ctx) {
    zval_dtor(static_cast<zval *>(ctx->private_data));
    efree(ctx->private_data);
}

void swoole_name_resolver_context_settle(void *context, zval *cluster, bool is_final) {
    auto *ctx = context_of(context);
    if (cluster) {
        ctx->private_data = cluster;
        ctx->dtor = free_cluster;
    }
    ctx->cluster_ = cluster != nullptr;
    ctx->final_ = is_final;
}

void swoole_name_resolver_context_release(void *context) {
    auto *ctx = context_of(context);
    ctx->dtor(ctx);
    ctx->private_data = nullptr;
}
