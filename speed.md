# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8bf6f8b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,137 | 15,219 | 0.9945× | 1.0× | ✅ |
| echo-1k | 18,050 | 18,136 | 0.9953× | 1.0× | ✅ |
| json | 23,146 | 23,151 | 0.9998× | 1.0× | ✅ |
| service-health | 17,399 | 17,802 | 0.9773× | 0.98× | ✅ |
| websocket | 7,105 | 8,188 | 0.8678× | 0.87× | ✅ |
| timer | 224,878 | 312,147 | 0.7204× | 0.72× | ✅ |
| app | 129,342 | 135,622 | 0.9537× | 0.96× | ✅ |
| http2 | 21,401 | 20,373 | 1.0505× | 1.05× | ❌ |
| table | 200,345 | 201,729 | 0.9931× | 1.0× | ✅ |
| channel | 141,115 | 154,640 | 0.9125× | 0.92× | ✅ |
| task | 34,068 | 33,179 | 1.0268× | 1.05× | ➖ |

Geometric mean 0.9492×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
