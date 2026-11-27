#include "swoole.h"
#include <gtest/gtest.h>

int main(int argc, char **argv) {
    SwooleG.std_allocator = {std::malloc, std::calloc, std::realloc, std::free};
    SwooleG.pagesize = 4096;
    testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
