# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c6d1bf2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,733 | 15,214 | 0.9027× | 0.91× | ✅ |
| echo-1k | 16,728 | 18,165 | 0.9209× | 0.92× | ✅ |
| json | 22,069 | 23,161 | 0.9529× | 0.95× | ✅ |
| service-health | 16,308 | 17,773 | 0.9176× | 0.92× | ✅ |
| websocket | 6,681 | 8,192 | 0.8155× | 0.8× | ✅ |
| timer | 201,541 | 313,732 | 0.6424× | 0.65× | ✅ |
| app | 126,785 | 135,841 | 0.9333× | 0.93× | ✅ |
| http2 | 18,968 | 20,496 | 0.9254× | 0.92× | ✅ |
| table | 193,446 | 202,900 | 0.9534× | 0.96× | ✅ |
| channel | 137,963 | 155,717 | 0.8860× | 0.88× | ✅ |
| task | 31,928 | 33,272 | 0.9596× | 0.96× | ✅ |
| mysql | 49,833 | 51,064 | 0.9759× | 0.97× | ✅ |
| http-client | 33,652 | 35,796 | 0.9401× | 0.93× | ✅ |
| gzip | 194,346 | 197,099 | 0.9860× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,764 | 0.9749× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,228 | 0.5078× | 0.5× | ✅ |
| udp | 7,583 | 7,986 | 0.9495× | 0.95× | ✅ |
| connect | 17,572 | 18,734 | 0.9380× | 0.93× | ✅ |
| https | 106,024 | 108,802 | 0.9745× | 0.98× | ✅ |
| wss | 101,243 | 104,797 | 0.9661× | 0.97× | ✅ |
| atomic | 50,167 | 51,272 | 0.9785× | 0.97× | ✅ |
| process-mode | 15,258 | 17,021 | 0.8964× | 0.89× | ✅ |
| http2-concurrent | 364,144 | 408,861 | 0.8906× | 0.89× | ✅ |
| websocket-64k | 89,975 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,844 | 16,368 | 0.7236× | 0.72× | ✅ |
| upload | 416,226 | 1,322,425 | 0.3147× | 0.32× | ✅ |
| static | 27,094 | 41,063 | 0.6598× | 0.65× | ✅ |
| client | 18,381 | 19,721 | 0.9321× | 0.93× | ✅ |
| socket | 19,628 | 21,036 | 0.9331× | 0.93× | ✅ |
| pgsql | 22,587 | 24,618 | 0.9175× | 0.92× | ✅ |
| sqlite | 23,401 | 25,236 | 0.9273× | 0.93× | ✅ |
| curl | 85,347 | 90,355 | 0.9446× | 0.95× | ✅ |
| dns | 229,898 | 232,562 | 0.9885× | 0.99× | ✅ |

Geometric mean 0.8531×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
