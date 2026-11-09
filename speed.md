# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `41db708` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,335 | 15,227 | 0.9414× | 1.0× | ✅ |
| echo-1k | 17,261 | 18,132 | 0.9520× | 1.0× | ✅ |
| json | 22,353 | 23,148 | 0.9657× | 1.0× | ✅ |
| service-health | 16,970 | 17,802 | 0.9533× | 0.98× | ✅ |
| websocket | 6,950 | 8,188 | 0.8488× | 0.87× | ✅ |
| timer | 223,936 | 311,854 | 0.7181× | 0.72× | ✅ |
| app | 127,514 | 135,475 | 0.9412× | 0.96× | ✅ |
| http2 | 19,581 | 20,373 | 0.9611× | 1.03× | ✅ |
| table | 199,099 | 201,729 | 0.9870× | 1.0× | ✅ |
| channel | 140,834 | 155,155 | 0.9077× | 0.92× | ✅ |
| task | 33,354 | 33,179 | 1.0053× | 1.03× | ➖ |
| mysql | 50,297 | 51,031 | 0.9856× | 1.1× | ✅ |

Geometric mean 0.9272×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
