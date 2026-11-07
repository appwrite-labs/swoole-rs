# Rust directory storage (`swr_v2_http_directory_*`)

`StaticHandler` keeps directory names in one opaque Rust owner. There is no
native copy of the collection, no side table and no exported Rust name pointer.
The handler cannot be copied.

## Exports

- `new` returns a unique owner or null. `drop` accepts null and destroys once.
- `insert` copies an initialized, disjoint span. Returns 1 = inserted,
  0 = duplicate, negative = INVALID / OVERFLOW / allocation (-3); nothing is
  published on error.
- `count` treats null as empty.
- `contains` treats null as empty and validates the span. Returns 1/0 or an error.
- `page` uses the index-page POD from `swr_v1_http_index_page`. Part 1 renders an
  owned entry; parts 0 and 2 accept a null owner. Query and fill are separate calls,
  and no borrow is held across native buffer growth.
- `scan_append` / `scan_finish` collect a directory scan (below).
- `clone` deep-copies an owner. `emit` is used by the Legacy store.

## Scanning

- The owner is allocated lazily on the first successful `readdir` entry. Plain file
  paths and empty scans allocate nothing.
- `scan_append` copies each name before the next `readdir` overwrites it. It
  publishes only complete names and keeps duplicates temporarily. While a scan is
  pending, only append, finish and drop are allowed; `insert`, `contains`, `count`
  and `page` reject a pending owner.
- After native `closedir`, `scan_finish` sorts by unsigned bytes and dedups in
  place, with no allocation. A null finish means an empty scan.
- A failed append publishes nothing. Names copied earlier stay owned and are freed
  on drop.
- Membership uses binary search. Index priority still follows the configured order.

## Page append

`append_directory_part` (V2) and Legacy `append_index_part` share one Rust append
operation. Rust handles empty-fragment admission, checked size arithmetic, the
choice between reserve and aligned growth, rendering, length publication and the
NUL rule. Native does only the String reserve/extend and publishes the returned
length. Header and entry parts reserve a NUL and use aligned growth for non-empty
buffers. The footer uses an exact reserve and adds no NUL. Filtered entries
allocate nothing. Legacy passes the size it queried before allocating back in on
continuation; V2 queries again on each call.

Known difference: allocation cadence, peak memory and destruction order differ
from native set nodes.

Tests: `crates/server/src/static_handler/tests.rs`. C++20 builds of the pinned SDK fail
`-Werror` on a deprecated volatile increment in `swoole_process_pool.h`
(upstream). Use C++14/17.
