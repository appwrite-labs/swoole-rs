# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `af61cd6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,132 | 15,219 | 0.9942× | 1.0× | ✅ |
| echo-1k | 18,066 | 18,146 | 0.9955× | 1.0× | ✅ |
| json | 23,149 | 23,148 | 1.0001× | 1.0× | ❌ |
| service-health | 17,402 | 17,802 | 0.9776× | 0.98× | ✅ |
| websocket | 7,104 | 8,191 | 0.8673× | 0.87× | ✅ |
| timer | 225,326 | 314,275 | 0.7170× | 0.72× | ✅ |
| app | 129,529 | 135,710 | 0.9545× | 0.96× | ✅ |
| http2 | 21,995 | 20,372 | 1.0797× | 1.05× | ❌ |
| table | 200,352 | 201,733 | 0.9932× | 1.0× | ✅ |
| channel | 141,115 | 154,639 | 0.9125× | 0.92× | ✅ |
| task | 34,059 | 33,179 | 1.0265× | 1.05× | ➖ |

Geometric mean 0.9512×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
