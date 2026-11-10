# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f694acb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,865 | 15,202 | 0.9121× | 0.91× | ✅ |
| echo-1k | 16,760 | 18,149 | 0.9235× | 0.92× | ✅ |
| json | 21,833 | 23,164 | 0.9425× | 0.94× | ✅ |
| service-health | 16,465 | 17,799 | 0.9251× | 0.93× | ✅ |
| websocket | 6,600 | 8,188 | 0.8061× | 0.81× | ✅ |
| timer | 210,558 | 311,520 | 0.6759× | 0.72× | ✅ |
| app | 125,355 | 135,686 | 0.9239× | 0.92× | ✅ |
| http2 | 19,302 | 20,342 | 0.9488× | 0.95× | ✅ |
| table | 197,183 | 201,681 | 0.9777× | 0.98× | ✅ |
| channel | 136,554 | 154,565 | 0.8835× | 0.89× | ✅ |
| task | 31,210 | 33,176 | 0.9407× | 0.94× | ✅ |
| mysql | 49,105 | 51,082 | 0.9613× | 0.97× | ✅ |

Geometric mean 0.8977×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
