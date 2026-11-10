# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `84e0b78` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,144 | 15,221 | 0.9292× | 0.91× | ✅ |
| echo-1k | 17,082 | 18,165 | 0.9404× | 0.92× | ✅ |
| json | 22,132 | 23,180 | 0.9548× | 0.94× | ✅ |
| service-health | 16,722 | 17,790 | 0.9400× | 0.93× | ✅ |
| websocket | 6,563 | 8,188 | 0.8016× | 0.81× | ✅ |
| timer | 207,631 | 313,702 | 0.6619× | 0.68× | ✅ |
| app | 125,163 | 135,818 | 0.9216× | 0.92× | ✅ |
| http2 | 19,491 | 20,386 | 0.9561× | 0.95× | ✅ |
| table | 196,984 | 201,693 | 0.9766× | 0.98× | ✅ |
| channel | 137,017 | 154,572 | 0.8864× | 0.89× | ✅ |
| task | 31,574 | 33,314 | 0.9478× | 0.94× | ✅ |
| mysql | 49,271 | 51,020 | 0.9657× | 0.97× | ✅ |

Geometric mean 0.9021×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
