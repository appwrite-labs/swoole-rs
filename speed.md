# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f5e2020` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,145 | 15,219 | 0.9951× | 1.0× | ✅ |
| echo-1k | 18,066 | 18,137 | 0.9961× | 1.0× | ✅ |
| json | 23,155 | 23,151 | 1.0002× | 1.0× | ❌ |
| service-health | 17,408 | 17,798 | 0.9781× | 0.98× | ✅ |
| websocket | 7,111 | 8,188 | 0.8684× | 0.87× | ✅ |
| timer | 225,129 | 313,453 | 0.7182× | 0.72× | ✅ |
| app | 130,162 | 135,814 | 0.9584× | 0.96× | ✅ |
| http2 | 21,403 | 20,369 | 1.0508× | 1.05× | ❌ |
| table | 200,354 | 201,729 | 0.9932× | 1.0× | ✅ |
| channel | 141,130 | 154,639 | 0.9126× | 0.92× | ✅ |
| task | 34,075 | 33,177 | 1.0270× | 1.05× | ➖ |

Geometric mean 0.9497×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
