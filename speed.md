# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8ba2343` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,806 | 15,204 | 0.9081× | 0.91× | ✅ |
| echo-1k | 16,685 | 18,149 | 0.9193× | 0.92× | ✅ |
| json | 21,755 | 23,158 | 0.9394× | 0.94× | ✅ |
| service-health | 16,405 | 17,788 | 0.9222× | 0.93× | ✅ |
| websocket | 6,605 | 8,184 | 0.8071× | 0.81× | ✅ |
| timer | 222,804 | 310,769 | 0.7169× | 0.72× | ✅ |
| app | 125,153 | 135,957 | 0.9205× | 0.92× | ✅ |
| http2 | 19,338 | 20,372 | 0.9492× | 0.95× | ✅ |
| table | 198,181 | 201,726 | 0.9824× | 0.99× | ✅ |
| channel | 136,551 | 155,152 | 0.8801× | 0.89× | ✅ |
| task | 31,131 | 33,177 | 0.9383× | 0.94× | ✅ |
| mysql | 49,064 | 51,084 | 0.9605× | 0.97× | ✅ |

Geometric mean 0.9007×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
