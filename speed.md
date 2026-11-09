# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `63079bc` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,126 | 15,219 | 0.9939× | 1.0× | ✅ |
| echo-1k | 18,054 | 18,136 | 0.9955× | 1.0× | ✅ |
| json | 23,143 | 23,151 | 0.9996× | 1.0× | ✅ |
| service-health | 17,396 | 17,802 | 0.9772× | 0.98× | ✅ |
| websocket | 7,109 | 8,188 | 0.8682× | 0.87× | ✅ |
| timer | 224,891 | 310,941 | 0.7233× | 0.72× | ✅ |
| app | 129,740 | 135,548 | 0.9572× | 0.96× | ✅ |
| http2 | 21,335 | 20,372 | 1.0473× | 1.05× | ➖ |
| table | 200,342 | 201,729 | 0.9931× | 1.0× | ✅ |
| channel | 141,105 | 154,640 | 0.9125× | 0.92× | ✅ |
| task | 34,588 | 33,174 | 1.0426× | 1.05× | ➖ |

Geometric mean 0.9509×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
