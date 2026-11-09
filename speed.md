# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3e38632` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,370 | 15,224 | 0.9439× | 1.0× | ✅ |
| echo-1k | 17,291 | 18,134 | 0.9535× | 1.0× | ✅ |
| json | 22,387 | 23,145 | 0.9673× | 1.0× | ✅ |
| service-health | 16,990 | 17,799 | 0.9545× | 0.98× | ✅ |
| websocket | 6,977 | 8,191 | 0.8518× | 0.87× | ✅ |
| timer | 223,707 | 310,798 | 0.7198× | 0.72× | ✅ |
| app | 127,865 | 135,617 | 0.9428× | 0.96× | ✅ |
| http2 | 19,636 | 20,370 | 0.9639× | 1.03× | ✅ |
| table | 199,123 | 201,729 | 0.9871× | 1.0× | ✅ |
| channel | 140,354 | 155,152 | 0.9046× | 0.92× | ✅ |
| task | 33,385 | 33,177 | 1.0062× | 1.03× | ➖ |
| mysql | 50,382 | 51,031 | 0.9873× | 1.1× | ✅ |

Geometric mean 0.9285×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
