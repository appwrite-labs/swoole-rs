# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `bf462c0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,743 | 15,210 | 0.9035× | 0.91× | ✅ |
| echo-1k | 16,743 | 18,162 | 0.9218× | 0.92× | ✅ |
| json | 22,072 | 23,173 | 0.9525× | 0.95× | ✅ |
| service-health | 16,318 | 17,782 | 0.9177× | 0.92× | ✅ |
| websocket | 6,681 | 8,192 | 0.8155× | 0.81× | ✅ |
| timer | 201,552 | 313,852 | 0.6422× | 0.65× | ✅ |
| app | 126,999 | 135,980 | 0.9339× | 0.93× | ✅ |
| http2 | 18,970 | 20,496 | 0.9255× | 0.92× | ✅ |
| table | 193,903 | 201,885 | 0.9605× | 0.96× | ✅ |
| channel | 137,969 | 155,723 | 0.8860× | 0.88× | ✅ |
| task | 31,947 | 33,269 | 0.9603× | 0.96× | ✅ |
| mysql | 50,069 | 51,086 | 0.9801× | 0.97× | ✅ |
| http-client | 33,402 | 35,790 | 0.9333× | 0.93× | ✅ |
| gzip | 194,361 | 197,099 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,764 | 0.9749× | 0.96× | ✅ |
| tcp-eof | 5,201 | 10,231 | 0.5083× | 0.5× | ✅ |
| udp | 7,579 | 7,986 | 0.9490× | 0.95× | ✅ |
| connect | 17,582 | 18,730 | 0.9387× | 0.93× | ✅ |
| https | 106,026 | 108,813 | 0.9744× | 0.98× | ✅ |
| wss | 101,240 | 104,800 | 0.9660× | 0.97× | ✅ |
| atomic | 50,173 | 51,285 | 0.9783× | 0.97× | ✅ |
| process-mode | 15,263 | 17,024 | 0.8966× | 0.89× | ✅ |
| http2-concurrent | 364,605 | 409,058 | 0.8913× | 0.89× | ✅ |
| websocket-64k | 89,981 | 159,691 | 0.5635× | 0.57× | ✅ |
| websocket-fragmented | 11,840 | 16,372 | 0.7232× | 0.72× | ✅ |
| upload | 416,435 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,059 | 40,999 | 0.6600× | 0.65× | ✅ |
| client | 18,472 | 19,730 | 0.9362× | 0.93× | ✅ |
| socket | 19,701 | 21,045 | 0.9361× | 0.93× | ✅ |
| pgsql | 22,917 | 24,617 | 0.9309× | 0.92× | ✅ |
| sqlite | 23,407 | 25,236 | 0.9275× | 0.93× | ✅ |
| curl | 85,219 | 90,487 | 0.9418× | 0.95× | ✅ |
| dns | 229,922 | 233,333 | 0.9854× | 0.99× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
