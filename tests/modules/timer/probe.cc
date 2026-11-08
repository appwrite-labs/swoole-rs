// Runs against the shipped Rust-owned timer; PHP supplies native runtime dependencies.
#include "swoole_timer.h"
#include <functional>
#include <iostream>
#include <memory>
#include <string>
#include <vector>
#include <sys/wait.h>
#include <unistd.h>

// A node's closures, its data; the node's callback and destructor call them.
struct Hooks {
    std::function<void(swoole::Timer *, swoole::TimerNode *)> call;
    std::function<void(swoole::TimerNode *)> destructor;
};
static Hooks &hooks(swoole::TimerNode *node) {
    return *static_cast<Hooks *>(node->data);
}
static void call(swoole::Timer *timer, swoole::TimerNode *node) {
    hooks(node).call(timer, node);
}
static void destruct(swoole::TimerNode *node) {
    std::unique_ptr<Hooks> owned(&hooks(node));
    owned->destructor(node);
}

extern "C" int swr_timer_probe(const char *name) {
    try {
        std::string scenario(name), events;
        unsigned destructors = 0;
        auto *timer = swr_v1_timer_owner_new(true); // Manual mode: no signals/reactor registrations.
        auto add = [&](char label, bool repeat, std::function<void(swoole::Timer *, swoole::TimerNode *)> callback) {
            auto *node = timer->add(1000, repeat, new Hooks{std::move(callback), nullptr}, call);
            if (!node) throw std::runtime_error("add failed");
            node->exec_msec = 0;
            timer->update(node); // Existing public deadline mutation hook, no clock replacement.
            node->destructor = destruct;
            hooks(node).destructor = [&, label](swoole::TimerNode *) { ++destructors; events += char(label + ('a' - 'A')); };
            return node;
        };
        auto record = [&](char label) { return [&, label](swoole::Timer *, swoole::TimerNode *) { events += label; }; };
        long a_id = 0, b_id = 0;
        swoole::TimerNode *other = nullptr;
        const bool repeat = scenario == "update-active-repeat";
        bool nested = false;
        if (scenario == "wrapper-clear") {
            auto *previous = SwooleTG.timer;
            SwooleTG.timer = timer;
            const auto inactive_id = add('C', false, record('C'))->id;
            const bool inactive = swoole_timer_clear(inactive_id) && !swoole_timer_clear(inactive_id);
            const bool missing = !swoole_timer_clear(0) && !swoole_timer_clear(-1) &&
                                 !swoole_timer_clear(inactive_id + 100);
            bool active = false, cancelled_other = false, active_view = false;
            long other_id = 0;
            add('A', true, [&](swoole::Timer *, swoole::TimerNode *node) {
                events += 'A';
                cancelled_other = swoole_timer_clear(other_id) && !swoole_timer_clear(other_id);
                active = swoole_timer_clear(node->id) && !swoole_timer_clear(node->id);
                active_view = swoole_timer_get(node->id) == node && node->removed;
            });
            other_id = add('B', false, record('B'))->id;
            timer->select();
            timer->select();
            const auto remaining = timer->count();
            swr_v1_timer_owner_free(timer);
            SwooleTG.timer = previous;
            std::cout << "{\"scenario\":\"wrapper-clear\",\"inactive\":" << (inactive ? "true" : "false")
                      << ",\"missing\":" << (missing ? "true" : "false")
                      << ",\"active\":" << (active ? "true" : "false")
                      << ",\"cancelled_other\":" << (cancelled_other ? "true" : "false")
                      << ",\"active_view\":" << (active_view ? "true" : "false")
                      << ",\"remaining\":" << remaining << ",\"events\":\"" << events
                      << "\",\"destructors\":" << destructors << "}\n";
            return 0;
        }
        if (scenario == "equal-deadlines") {
            add('A', false, record('A'));
            add('B', false, record('B'));
            add('C', false, record('C'));
        } else if (scenario == "self-remove") {
            add('A', true, [&](swoole::Timer *t, swoole::TimerNode *n) {
                events += 'A';
                events += t->remove(n) ? 'Y' : 'N';
                events += t->remove(n) ? 'Y' : 'N';
                events += t->get(n->id) == n && n->removed ? 'V' : 'X';
            });
        } else if (scenario == "remove-other" || scenario == "destructor-remove-other") {
            auto *a = add('A', false, [&](swoole::Timer *t, swoole::TimerNode *) {
                events += 'A';
                if (scenario == "remove-other") t->remove(other);
            });
            other = add('B', false, record('B'));
            add('C', false, record('C'));
            if (scenario == "destructor-remove-other") {
                hooks(a).destructor = [&](swoole::TimerNode *) { ++destructors; events += 'a'; timer->remove(other); };
            }
        } else if (scenario == "delay-active-once" || scenario == "update-active-repeat") {
            auto *a = add('A', repeat, [&](swoole::Timer *t, swoole::TimerNode *n) {
                events += 'A';
                if (repeat) { n->exec_msec = 100000; t->update(n); }
                else t->delay(n, 100000);
                std::cout << "{\"phase\":\"callback-updated\",\"scenario\":\"" << scenario << "\"}\n" << std::flush;
            });
            a_id = a->id;
            b_id = add('B', false, record('B'))->id;
        } else if (scenario == "destructor-add") {
            auto *a = add('A', false, record('A'));
            hooks(a).destructor = [&](swoole::TimerNode *) { ++destructors; events += 'a'; add('C', false, record('C')); };
            add('B', false, record('B'));
        } else if (scenario == "nested-select-once") {
            add('A', false, [&](swoole::Timer *t, swoole::TimerNode *) {
                events += 'A';
                if (!nested) { nested = true; t->select(); }
            });
        } else if (scenario == "fork-active-teardown") {
            bool parent_intact = false;
            int child_status = -1;
            add('A', false, [&](swoole::Timer *t, swoole::TimerNode *node) {
                const auto child = fork();
                if (child < 0) throw std::runtime_error("fork failed");
                if (child == 0) {
                    // Copy references before deleting the callback's own closure.
                    auto *count = &destructors;
                    auto *log = &events;
                    swr_v1_timer_owner_free(t);
                    _exit(*count == 2 && log->size() == 2 ? 0 : 2);
                }
                if (waitpid(child, &child_status, 0) != child) throw std::runtime_error("waitpid failed");
                parent_intact = t->get(node->id) == node && t->count() == 2 && destructors == 0;
                events += 'A';
            });
            add('B', false, record('B'));
            timer->select();
            timer->select();
            const bool parent_drained = timer->count() == 0 && events == "AaBb" && destructors == 2;
            swr_v1_timer_owner_free(timer);
            std::cout << "{\"scenario\":\"fork-active-teardown\",\"child_status\":" << child_status
                      << ",\"parent_intact\":" << (parent_intact ? "true" : "false")
                      << ",\"parent_drained\":" << (parent_drained ? "true" : "false")
                      << ",\"parent_destructors\":" << destructors << "}\n";
            return 0;
        } else if (scenario == "fork-teardown") {
            const auto id = add('A', false, record('A'))->id;
            const auto child = fork();
            if (child < 0) throw std::runtime_error("fork failed");
            if (child == 0) { swr_v1_timer_owner_free(timer); _exit(destructors == 1 ? 0 : 2); }
            int status;
            if (waitpid(child, &status, 0) != child) throw std::runtime_error("waitpid failed");
            const bool parent_intact = timer->get(id) && timer->count() == 1 && destructors == 0;
            swr_v1_timer_owner_free(timer);
            std::cout << "{\"scenario\":\"fork-teardown\",\"child_status\":" << status
                      << ",\"parent_intact\":" << (parent_intact ? "true" : "false")
                      << ",\"parent_destructors\":" << destructors << "}\n";
            return 0;
        } else throw std::runtime_error("unknown scenario");
        timer->select(); // Creation-round barrier; advancing selection round is observable behavior.
        const std::string after_first = events;
        timer->select();
        const auto count = timer->count();
        const bool a_present = a_id && timer->get(a_id);
        const bool b_present = b_id && timer->get(b_id);
        const std::string after_second = events;
        if (scenario == "destructor-add") timer->select();
        swr_v1_timer_owner_free(timer);
        std::cout << "{\"scenario\":\"" << scenario << "\",\"after_first\":\"" << after_first
                  << "\",\"after_second\":\"" << after_second << "\",\"events\":\"" << events
                  << "\",\"count_after_second\":" << count << ",\"destructors\":" << destructors
                  << ",\"a_present\":" << (a_present ? "true" : "false")
                  << ",\"b_present\":" << (b_present ? "true" : "false") << "}\n";
        return 0;
    } catch (const std::exception &error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
