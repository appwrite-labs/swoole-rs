# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `1e166ca` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,339 | 15,227 | 0.9417× | 1.0× | ✅ |
| echo-1k | 17,257 | 18,142 | 0.9512× | 1.0× | ✅ |
| json | 22,349 | 23,151 | 0.9654× | 1.0× | ✅ |
| service-health | 16,966 | 17,802 | 0.9531× | 0.98× | ✅ |
| websocket | 6,946 | 8,188 | 0.8484× | 0.87× | ✅ |
| timer | 223,916 | 311,195 | 0.7195× | 0.72× | ✅ |
| app | 127,737 | 135,777 | 0.9408× | 0.96× | ✅ |
| http2 | 19,580 | 20,372 | 0.9611× | 1.03× | ✅ |
| table | 199,095 | 201,729 | 0.9869× | 1.0× | ✅ |
| channel | 140,834 | 155,156 | 0.9077× | 0.92× | ✅ |
| task | 33,357 | 33,180 | 1.0053× | 1.03× | ➖ |
| mysql | 50,297 | 51,033 | 0.9856× | 1.1× | ✅ |

Geometric mean 0.9272×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
