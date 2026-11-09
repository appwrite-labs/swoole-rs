# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `45c3e44` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,096 | 15,219 | 0.9919× | 1.0× | ✅ |
| echo-1k | 17,977 | 18,136 | 0.9913× | 1.0× | ✅ |
| json | 23,110 | 23,151 | 0.9982× | 1.0× | ✅ |
| service-health | 17,354 | 17,802 | 0.9748× | 0.98× | ✅ |
| websocket | 7,100 | 8,191 | 0.8668× | 0.87× | ✅ |
| timer | 225,683 | 314,134 | 0.7184× | 0.72× | ✅ |
| app | 129,522 | 135,821 | 0.9536× | 0.96× | ✅ |
| http2 | 22,341 | 20,372 | 1.0966× | 1.1× | ➖ |
| table | 241,125 | 201,729 | 1.1953× | 1.2× | ➖ |
| channel | 141,066 | 154,639 | 0.9122× | 0.92× | ✅ |
| task | 43,251 | 33,180 | 1.3035× | 1.31× | ➖ |

Geometric mean 0.9890×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
