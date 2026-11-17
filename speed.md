# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0150ade` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,797 | 15,210 | 0.9071× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,162 | 0.9258× | 0.93× | ✅ |
| json | 21,957 | 23,161 | 0.9480× | 0.95× | ✅ |
| service-health | 16,370 | 17,773 | 0.9210× | 0.92× | ✅ |
| websocket | 6,554 | 8,191 | 0.8001× | 0.8× | ✅ |
| timer | 205,959 | 313,528 | 0.6569× | 0.65× | ✅ |
| app | 126,956 | 135,925 | 0.9340× | 0.93× | ✅ |
| http2 | 19,140 | 20,464 | 0.9353× | 0.92× | ✅ |
| table | 194,374 | 201,865 | 0.9629× | 0.97× | ✅ |
| channel | 137,575 | 155,171 | 0.8866× | 0.88× | ✅ |
| task | 32,506 | 33,268 | 0.9771× | 0.96× | ✅ |
| mysql | 50,032 | 51,059 | 0.9799× | 0.97× | ✅ |
| http-client | 33,714 | 35,800 | 0.9417× | 0.93× | ✅ |
| gzip | 194,454 | 197,079 | 0.9867× | 0.99× | ✅ |
| websocket-deflate | 179,003 | 181,564 | 0.9859× | 0.99× | ✅ |
| tcp-length | 4,586 | 4,765 | 0.9623× | 0.96× | ✅ |
| tcp-eof | 5,166 | 10,231 | 0.5049× | 0.5× | ✅ |
| udp | 7,487 | 7,986 | 0.9375× | 0.93× | ✅ |
| connect | 17,496 | 18,707 | 0.9353× | 0.93× | ✅ |
| https | 105,968 | 108,806 | 0.9739× | 0.98× | ✅ |
| wss | 100,992 | 104,800 | 0.9637× | 0.97× | ✅ |
| atomic | 49,877 | 51,312 | 0.9720× | 0.97× | ✅ |
| process-mode | 15,046 | 16,956 | 0.8874× | 0.88× | ✅ |
| http2-concurrent | 366,310 | 409,073 | 0.8955× | 0.88× | ✅ |
| websocket-64k | 89,803 | 159,695 | 0.5623× | 0.57× | ✅ |
| websocket-fragmented | 11,624 | 16,327 | 0.7119× | 0.72× | ✅ |
| upload | 415,385 | 1,322,388 | 0.3141× | 0.32× | ✅ |
| static | 26,673 | 41,051 | 0.6498× | 0.65× | ✅ |
| client | 18,622 | 19,754 | 0.9427× | 0.94× | ✅ |
| socket | 19,762 | 21,072 | 0.9378× | 0.92× | ✅ |
| pgsql | 22,819 | 24,595 | 0.9278× | 0.93× | ✅ |
| sqlite | 23,467 | 25,236 | 0.9299× | 0.93× | ✅ |
| curl | 86,423 | 90,451 | 0.9555× | 0.94× | ✅ |
| dns | 230,421 | 233,223 | 0.9880× | 0.99× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
