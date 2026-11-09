# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `43a48b9` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,123 | 15,227 | 0.9275× | 1.0× | ✅ |
| echo-1k | 17,079 | 18,144 | 0.9413× | 1.0× | ✅ |
| json | 22,133 | 23,151 | 0.9560× | 1.0× | ✅ |
| service-health | 16,736 | 17,802 | 0.9402× | 0.98× | ✅ |
| websocket | 6,705 | 8,191 | 0.8185× | 0.87× | ✅ |
| timer | 223,735 | 311,438 | 0.7184× | 0.72× | ✅ |
| app | 126,253 | 135,754 | 0.9300× | 0.96× | ✅ |
| http2 | 19,366 | 20,369 | 0.9508× | 1.03× | ✅ |
| table | 198,876 | 201,729 | 0.9859× | 1.0× | ✅ |
| channel | 137,926 | 155,152 | 0.8890× | 0.92× | ✅ |
| task | 33,421 | 33,175 | 1.0074× | 1.03× | ➖ |
| mysql | 50,078 | 51,031 | 0.9813× | 1.1× | ✅ |

Geometric mean 0.9170×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
