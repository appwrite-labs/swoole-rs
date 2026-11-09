# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e16002a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,789 | 15,203 | 0.9070× | 0.91× | ✅ |
| echo-1k | 16,649 | 18,145 | 0.9176× | 0.92× | ✅ |
| json | 21,756 | 23,163 | 0.9393× | 0.95× | ✅ |
| service-health | 16,391 | 17,791 | 0.9213× | 0.93× | ✅ |
| websocket | 6,600 | 8,191 | 0.8057× | 0.81× | ✅ |
| timer | 222,635 | 311,402 | 0.7149× | 0.73× | ✅ |
| app | 124,938 | 135,797 | 0.9200× | 0.93× | ✅ |
| http2 | 19,294 | 20,342 | 0.9485× | 0.95× | ✅ |
| table | 198,137 | 201,681 | 0.9824× | 0.99× | ✅ |
| channel | 136,480 | 154,568 | 0.8830× | 0.89× | ✅ |
| task | 31,089 | 33,179 | 0.9370× | 0.94× | ✅ |
| mysql | 49,037 | 51,072 | 0.9601× | 0.97× | ✅ |

Geometric mean 0.9000×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
