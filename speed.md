# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d0ac6a1` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,135 | 15,216 | 0.9947× | 1.0× | ✅ |
| echo-1k | 18,078 | 18,134 | 0.9969× | 1.0× | ✅ |
| json | 23,149 | 23,151 | 0.9999× | 1.0× | ✅ |
| service-health | 17,399 | 17,802 | 0.9773× | 0.98× | ✅ |
| websocket | 7,104 | 8,191 | 0.8673× | 0.87× | ✅ |
| timer | 225,468 | 314,253 | 0.7175× | 0.72× | ✅ |
| app | 129,508 | 135,729 | 0.9542× | 0.96× | ✅ |
| http2 | 21,430 | 20,372 | 1.0519× | 1.05× | ❌ |
| table | 200,352 | 201,733 | 0.9932× | 1.0× | ✅ |
| channel | 141,115 | 154,639 | 0.9125× | 0.92× | ✅ |
| task | 34,068 | 33,181 | 1.0267× | 1.05× | ➖ |

Geometric mean 0.9491×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
