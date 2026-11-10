# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e37e5c5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,865 | 15,241 | 0.9097× | 0.91× | ✅ |
| echo-1k | 16,748 | 18,199 | 0.9203× | 0.92× | ✅ |
| json | 21,837 | 23,200 | 0.9413× | 0.94× | ✅ |
| service-health | 16,469 | 17,835 | 0.9234× | 0.93× | ✅ |
| websocket | 6,600 | 8,227 | 0.8022× | 0.81× | ✅ |
| timer | 210,388 | 311,064 | 0.6763× | 0.68× | ✅ |
| app | 125,290 | 136,114 | 0.9205× | 0.92× | ✅ |
| http2 | 19,302 | 20,379 | 0.9471× | 0.95× | ✅ |
| table | 197,183 | 201,721 | 0.9775× | 0.98× | ✅ |
| channel | 136,554 | 154,643 | 0.8830× | 0.89× | ✅ |
| task | 31,207 | 33,178 | 0.9406× | 0.94× | ✅ |
| mysql | 49,113 | 51,117 | 0.9608× | 0.97× | ✅ |

Geometric mean 0.8962×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
