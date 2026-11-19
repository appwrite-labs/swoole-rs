# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `32bc813` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,748 | 15,214 | 0.9037× | 0.91× | ✅ |
| echo-1k | 16,760 | 18,165 | 0.9226× | 0.92× | ✅ |
| json | 22,076 | 23,173 | 0.9527× | 0.95× | ✅ |
| service-health | 16,331 | 17,786 | 0.9182× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8152× | 0.81× | ✅ |
| timer | 201,463 | 313,837 | 0.6419× | 0.65× | ✅ |
| app | 126,982 | 135,961 | 0.9340× | 0.93× | ✅ |
| http2 | 18,966 | 20,498 | 0.9253× | 0.92× | ✅ |
| table | 194,301 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,353 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 31,964 | 33,269 | 0.9608× | 0.96× | ✅ |
| mysql | 50,050 | 51,085 | 0.9797× | 0.97× | ✅ |
| http-client | 33,375 | 35,796 | 0.9323× | 0.93× | ✅ |
| gzip | 194,366 | 197,095 | 0.9862× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,763 | 0.9754× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,228 | 0.5080× | 0.5× | ✅ |
| udp | 7,560 | 7,987 | 0.9465× | 0.95× | ✅ |
| connect | 17,588 | 18,730 | 0.9390× | 0.93× | ✅ |
| https | 106,004 | 108,806 | 0.9743× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,187 | 51,285 | 0.9786× | 0.97× | ✅ |
| process-mode | 15,271 | 17,024 | 0.8970× | 0.89× | ✅ |
| http2-concurrent | 364,835 | 409,053 | 0.8919× | 0.89× | ✅ |
| websocket-64k | 89,978 | 159,692 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,835 | 16,372 | 0.7229× | 0.72× | ✅ |
| upload | 416,420 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,103 | 41,092 | 0.6596× | 0.65× | ✅ |
| client | 18,479 | 19,730 | 0.9366× | 0.93× | ✅ |
| socket | 19,705 | 21,046 | 0.9363× | 0.93× | ✅ |
| pgsql | 23,111 | 24,663 | 0.9370× | 0.92× | ✅ |
| sqlite | 23,442 | 25,268 | 0.9277× | 0.93× | ✅ |
| curl | 85,928 | 90,423 | 0.9503× | 0.95× | ✅ |
| dns | 229,915 | 232,573 | 0.9886× | 0.99× | ✅ |
| file | 108,202 | 131,300 | 0.8241× | 0.83× | ✅ |

Geometric mean 0.8533×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
