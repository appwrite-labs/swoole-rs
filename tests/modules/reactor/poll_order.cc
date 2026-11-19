// SPDX-License-Identifier: Apache-2.0
#include <string>
#include <iostream>
#include <unordered_map>

int main(int argc, char **argv) {
    std::unordered_map<int, int> fds;
    for (int i = 1; i < argc; ++i) {
        fds[std::stoi(argv[i])] |= 1;
    }
    for (const auto &fd : fds) {
        std::cout << fd.first << ' ';
    }
    std::cout << '\n';

    std::unordered_map<int, int> growth;
    for (int fd = 0; fd <= 1447153; ++fd) {
        const auto before = growth.bucket_count();
        growth.emplace(fd, 0);
        if (growth.bucket_count() != before) {
            std::cout << growth.bucket_count() << ' ';
        }
    }
    std::cout << '\n';
}
