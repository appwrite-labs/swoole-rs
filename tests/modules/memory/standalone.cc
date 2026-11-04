// Standalone native-runtime witnesses: no PHP loader or Zend frame is involved.
#include "swoole_table.h"
#include <cassert>
#include <cstring>

extern "C" int swr_memory_probe();
extern "C" int swr_memory_mapping_probe();
#ifdef SWOOLE_RS_MEMORY
extern "C" int swr_memory_odd_table_probe();
#endif

int main(int argc, char **argv) {
    if (argc > 2) return 2;
    swoole_init();
    int result = 2;
    if (argc == 1) result = swr_memory_probe();
#ifdef SWOOLE_RS_MEMORY
    else if (strcmp(argv[1], "mapping") == 0) result = swr_memory_mapping_probe();
    else if (strcmp(argv[1], "odd-rows") == 0) result = swr_memory_odd_table_probe();
#endif
    swoole_clean();
    return result;
}
