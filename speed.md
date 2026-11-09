# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8eb7527` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,812 | 15,206 | 0.9083× | 0.91× | ✅ |
| echo-1k | 16,697 | 18,150 | 0.9199× | 0.92× | ✅ |
| json | 21,767 | 23,159 | 0.9399× | 0.94× | ✅ |
| service-health | 16,400 | 17,795 | 0.9216× | 0.93× | ✅ |
| websocket | 6,607 | 8,188 | 0.8070× | 0.81× | ✅ |
| timer | 222,663 | 311,200 | 0.7155× | 0.72× | ✅ |
| app | 125,190 | 135,779 | 0.9220× | 0.92× | ✅ |
| http2 | 19,326 | 20,342 | 0.9501× | 0.95× | ✅ |
| table | 198,146 | 201,681 | 0.9825× | 0.99× | ✅ |
| channel | 136,492 | 154,565 | 0.8831× | 0.89× | ✅ |
| task | 31,147 | 33,176 | 0.9388× | 0.94× | ✅ |
| mysql | 49,064 | 51,076 | 0.9606× | 0.97× | ✅ |

Geometric mean 0.9011×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
