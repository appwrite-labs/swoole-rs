# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `cbd5b3f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,096 | 15,216 | 0.9921× | 1.0× | ✅ |
| echo-1k | 17,979 | 18,138 | 0.9913× | 1.0× | ✅ |
| json | 23,110 | 23,151 | 0.9982× | 1.0× | ✅ |
| service-health | 17,357 | 17,798 | 0.9752× | 0.98× | ✅ |
| websocket | 7,104 | 8,188 | 0.8676× | 0.87× | ✅ |
| timer | 225,495 | 314,216 | 0.7176× | 0.72× | ✅ |
| app | 129,560 | 135,676 | 0.9549× | 0.96× | ✅ |
| http2 | 22,336 | 20,369 | 1.0966× | 1.1× | ➖ |
| table | 200,303 | 201,729 | 0.9929× | 1.0× | ✅ |
| channel | 141,076 | 154,639 | 0.9123× | 0.92× | ✅ |
| task | 35,015 | 33,186 | 1.0551× | 1.06× | ➖ |

Geometric mean 0.9541×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
