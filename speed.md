# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `aaa51b2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,990 | 15,224 | 0.9189× | 1.0× | ✅ |
| echo-1k | 16,907 | 18,138 | 0.9322× | 1.0× | ✅ |
| json | 21,994 | 23,151 | 0.9500× | 1.0× | ✅ |
| service-health | 16,601 | 17,798 | 0.9327× | 0.98× | ✅ |
| websocket | 6,586 | 8,191 | 0.8040× | 0.87× | ✅ |
| timer | 222,197 | 310,962 | 0.7145× | 0.72× | ✅ |
| app | 125,809 | 135,845 | 0.9261× | 0.96× | ✅ |
| http2 | 19,148 | 20,369 | 0.9401× | 1.03× | ✅ |
| table | 197,851 | 201,730 | 0.9808× | 1.0× | ✅ |
| channel | 137,510 | 155,152 | 0.8863× | 0.92× | ✅ |
| task | 31,481 | 33,174 | 0.9490× | 1.03× | ✅ |
| mysql | 49,528 | 51,031 | 0.9706× | 1.1× | ✅ |

Geometric mean 0.9055×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
