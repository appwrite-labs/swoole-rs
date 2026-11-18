# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `be312b1` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,742 | 15,214 | 0.9033× | 0.91× | ✅ |
| echo-1k | 16,739 | 18,165 | 0.9215× | 0.92× | ✅ |
| json | 22,072 | 23,170 | 0.9526× | 0.95× | ✅ |
| service-health | 16,318 | 17,782 | 0.9177× | 0.92× | ✅ |
| websocket | 6,681 | 8,189 | 0.8158× | 0.81× | ✅ |
| timer | 201,360 | 313,869 | 0.6415× | 0.65× | ✅ |
| app | 127,016 | 135,968 | 0.9342× | 0.93× | ✅ |
| http2 | 18,976 | 20,496 | 0.9258× | 0.92× | ✅ |
| table | 193,903 | 201,885 | 0.9605× | 0.96× | ✅ |
| channel | 137,973 | 155,723 | 0.8860× | 0.88× | ✅ |
| task | 31,946 | 33,272 | 0.9602× | 0.96× | ✅ |
| mysql | 50,069 | 51,080 | 0.9802× | 0.97× | ✅ |
| http-client | 33,398 | 35,793 | 0.9331× | 0.93× | ✅ |
| gzip | 194,361 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,766 | 0.9744× | 0.96× | ✅ |
| tcp-eof | 5,201 | 10,228 | 0.5085× | 0.5× | ✅ |
| udp | 7,579 | 7,987 | 0.9489× | 0.95× | ✅ |
| connect | 17,582 | 18,730 | 0.9387× | 0.93× | ✅ |
| https | 106,026 | 108,813 | 0.9744× | 0.98× | ✅ |
| wss | 101,240 | 104,800 | 0.9660× | 0.97× | ✅ |
| atomic | 50,173 | 51,276 | 0.9785× | 0.97× | ✅ |
| process-mode | 15,264 | 17,021 | 0.8967× | 0.89× | ✅ |
| http2-concurrent | 364,130 | 408,865 | 0.8906× | 0.89× | ✅ |
| websocket-64k | 89,978 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,840 | 16,372 | 0.7232× | 0.72× | ✅ |
| upload | 416,434 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,070 | 41,028 | 0.6598× | 0.65× | ✅ |
| client | 18,472 | 19,720 | 0.9367× | 0.93× | ✅ |
| socket | 19,701 | 21,036 | 0.9365× | 0.93× | ✅ |
| pgsql | 22,926 | 24,618 | 0.9313× | 0.92× | ✅ |
| sqlite | 23,415 | 25,236 | 0.9278× | 0.93× | ✅ |
| curl | 85,396 | 90,359 | 0.9451× | 0.95× | ✅ |
| dns | 229,472 | 232,974 | 0.9850× | 0.99× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
