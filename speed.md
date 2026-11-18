# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c6d1bf2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,737 | 15,213 | 0.9029× | 0.91× | ✅ |
| echo-1k | 16,728 | 18,162 | 0.9210× | 0.92× | ✅ |
| json | 22,062 | 23,173 | 0.9520× | 0.95× | ✅ |
| service-health | 16,308 | 17,786 | 0.9169× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8153× | 0.8× | ✅ |
| timer | 201,414 | 313,828 | 0.6418× | 0.65× | ✅ |
| app | 126,756 | 135,865 | 0.9330× | 0.93× | ✅ |
| http2 | 18,971 | 20,496 | 0.9256× | 0.92× | ✅ |
| table | 193,897 | 201,885 | 0.9604× | 0.96× | ✅ |
| channel | 138,479 | 155,726 | 0.8892× | 0.88× | ✅ |
| task | 31,935 | 33,269 | 0.9599× | 0.96× | ✅ |
| mysql | 49,833 | 51,061 | 0.9760× | 0.97× | ✅ |
| http-client | 33,656 | 35,796 | 0.9402× | 0.93× | ✅ |
| gzip | 194,346 | 197,099 | 0.9860× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,766 | 0.9744× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,231 | 0.5077× | 0.5× | ✅ |
| udp | 7,583 | 7,987 | 0.9495× | 0.95× | ✅ |
| connect | 17,572 | 18,730 | 0.9382× | 0.93× | ✅ |
| https | 106,028 | 108,809 | 0.9744× | 0.98× | ✅ |
| wss | 101,243 | 104,797 | 0.9661× | 0.97× | ✅ |
| atomic | 50,167 | 51,276 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,255 | 17,021 | 0.8962× | 0.89× | ✅ |
| http2-concurrent | 364,161 | 408,861 | 0.8907× | 0.89× | ✅ |
| websocket-64k | 89,979 | 159,691 | 0.5635× | 0.57× | ✅ |
| websocket-fragmented | 11,840 | 16,368 | 0.7234× | 0.72× | ✅ |
| upload | 416,221 | 1,322,425 | 0.3147× | 0.32× | ✅ |
| static | 27,094 | 41,063 | 0.6598× | 0.65× | ✅ |
| client | 18,382 | 19,721 | 0.9321× | 0.93× | ✅ |
| socket | 19,631 | 21,033 | 0.9334× | 0.93× | ✅ |
| pgsql | 22,587 | 24,618 | 0.9175× | 0.92× | ✅ |
| sqlite | 23,408 | 25,236 | 0.9276× | 0.93× | ✅ |
| curl | 85,346 | 90,357 | 0.9445× | 0.95× | ✅ |
| dns | 229,925 | 232,905 | 0.9872× | 0.99× | ✅ |

Geometric mean 0.8533×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
