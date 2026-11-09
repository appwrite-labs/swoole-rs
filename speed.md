# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0896e8c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,135 | 15,219 | 0.9945× | 1.0× | ✅ |
| echo-1k | 18,059 | 18,137 | 0.9957× | 1.0× | ✅ |
| json | 23,149 | 23,151 | 0.9999× | 1.0× | ✅ |
| service-health | 17,402 | 17,802 | 0.9776× | 0.98× | ✅ |
| websocket | 7,104 | 8,188 | 0.8676× | 0.87× | ✅ |
| timer | 225,487 | 314,262 | 0.7175× | 0.72× | ✅ |
| app | 129,492 | 135,693 | 0.9543× | 0.96× | ✅ |
| http2 | 21,996 | 20,369 | 1.0799× | 1.05× | ❌ |
| table | 200,348 | 201,729 | 0.9932× | 1.0× | ✅ |
| channel | 141,119 | 154,639 | 0.9126× | 0.92× | ✅ |
| task | 34,069 | 33,172 | 1.0270× | 1.05× | ➖ |

Geometric mean 0.9514×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
