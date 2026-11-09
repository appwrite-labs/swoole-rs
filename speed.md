# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `167ca5d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,826 | 15,203 | 0.9094× | 0.93× | ✅ |
| echo-1k | 16,677 | 18,145 | 0.9191× | 0.94× | ✅ |
| json | 21,774 | 23,159 | 0.9402× | 0.96× | ✅ |
| service-health | 16,410 | 17,795 | 0.9222× | 0.94× | ✅ |
| websocket | 6,576 | 8,188 | 0.8032× | 0.82× | ✅ |
| timer | 210,797 | 311,732 | 0.6762× | 0.71× | ✅ |
| app | 125,069 | 135,721 | 0.9215× | 0.93× | ✅ |
| http2 | 19,269 | 20,341 | 0.9473× | 0.95× | ✅ |
| table | 197,655 | 201,681 | 0.9800× | 0.99× | ✅ |
| channel | 136,507 | 154,565 | 0.8832× | 0.89× | ✅ |
| task | 31,104 | 33,177 | 0.9375× | 0.96× | ✅ |
| mysql | 49,064 | 51,072 | 0.9607× | 0.98× | ✅ |

Geometric mean 0.8960×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
