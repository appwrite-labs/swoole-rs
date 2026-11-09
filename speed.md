# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b3dd374` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,122 | 15,227 | 0.9274× | 1.0× | ✅ |
| echo-1k | 17,063 | 18,139 | 0.9407× | 1.0× | ✅ |
| json | 22,137 | 23,148 | 0.9563× | 1.0× | ✅ |
| service-health | 16,740 | 17,798 | 0.9405× | 0.98× | ✅ |
| websocket | 6,705 | 8,191 | 0.8185× | 0.87× | ✅ |
| timer | 223,806 | 311,127 | 0.7193× | 0.72× | ✅ |
| app | 126,769 | 135,824 | 0.9333× | 0.96× | ✅ |
| http2 | 19,370 | 20,373 | 0.9508× | 1.03× | ✅ |
| table | 198,873 | 201,726 | 0.9859× | 1.0× | ✅ |
| channel | 137,922 | 155,152 | 0.8889× | 0.92× | ✅ |
| task | 33,417 | 33,175 | 1.0073× | 1.03× | ➖ |
| mysql | 50,078 | 51,031 | 0.9813× | 1.1× | ✅ |

Geometric mean 0.9174×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
