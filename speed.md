# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `40ce7df` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,142 | 15,219 | 0.9949× | 1.0× | ✅ |
| echo-1k | 18,068 | 18,140 | 0.9960× | 1.0× | ✅ |
| json | 23,155 | 23,151 | 1.0002× | 1.0× | ❌ |
| service-health | 17,408 | 17,802 | 0.9779× | 0.98× | ✅ |
| websocket | 7,111 | 8,188 | 0.8684× | 0.87× | ✅ |
| timer | 225,455 | 314,174 | 0.7176× | 0.72× | ✅ |
| app | 130,084 | 135,482 | 0.9602× | 0.96× | ✅ |
| http2 | 21,408 | 20,369 | 1.0510× | 1.05× | ❌ |
| table | 200,354 | 201,729 | 0.9932× | 1.0× | ✅ |
| channel | 141,131 | 154,639 | 0.9126× | 0.92× | ✅ |
| task | 34,082 | 33,186 | 1.0270× | 1.05× | ➖ |

Geometric mean 0.9498×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
