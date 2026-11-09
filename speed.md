# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5d2df37` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,153 | 15,216 | 0.9959× | 1.0× | ✅ |
| echo-1k | 18,085 | 18,137 | 0.9971× | 1.0× | ✅ |
| json | 23,166 | 23,151 | 1.0007× | 1.0× | ❌ |
| service-health | 17,420 | 17,798 | 0.9787× | 0.98× | ✅ |
| websocket | 7,114 | 8,191 | 0.8684× | 0.87× | ✅ |
| timer | 224,596 | 311,121 | 0.7219× | 0.72× | ✅ |
| app | 129,601 | 135,699 | 0.9551× | 0.96× | ✅ |
| http2 | 21,415 | 20,370 | 1.0513× | 1.03× | ❌ |
| table | 200,361 | 201,730 | 0.9932× | 1.0× | ✅ |
| channel | 141,138 | 154,636 | 0.9127× | 0.92× | ✅ |
| task | 34,062 | 33,172 | 1.0268× | 1.03× | ➖ |

Geometric mean 0.9502×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
