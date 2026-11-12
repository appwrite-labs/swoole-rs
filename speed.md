# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c583306` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,913 | 15,213 | 0.9145× | 0.92× | ✅ |
| echo-1k | 17,014 | 18,166 | 0.9366× | 0.94× | ✅ |
| json | 22,019 | 23,161 | 0.9507× | 0.95× | ✅ |
| service-health | 16,520 | 17,777 | 0.9293× | 0.93× | ✅ |
| websocket | 6,573 | 8,188 | 0.8027× | 0.82× | ✅ |
| timer | 203,259 | 313,668 | 0.6480× | 0.65× | ✅ |
| app | 125,758 | 135,963 | 0.9249× | 0.93× | ✅ |
| http2 | 18,656 | 20,464 | 0.9116× | 0.93× | ✅ |
| table | 195,473 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 137,920 | 154,654 | 0.8918× | 0.9× | ✅ |
| task | 32,149 | 33,268 | 0.9664× | 0.96× | ✅ |
| mysql | 49,589 | 51,061 | 0.9712× | 0.97× | ✅ |
| http-client | 33,675 | 35,805 | 0.9405× | 0.94× | ✅ |
| client | 18,482 | 19,756 | 0.9355× | 0.94× | ✅ |
| socket | 19,597 | 21,067 | 0.9302× | 0.92× | ✅ |
| gzip | 195,985 | 197,079 | 0.9944× | 1.0× | ✅ |
| websocket-deflate | 181,340 | 181,572 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,786 | 4,765 | 1.0043× | 1.05× | ➖ |
| tcp-eof | 5,334 | 10,231 | 0.5213× | 0.54× | ✅ |
| udp | 7,575 | 7,986 | 0.9485× | 0.95× | ✅ |
| connect | 18,345 | 18,707 | 0.9806× | 0.98× | ✅ |
| https | 108,775 | 108,813 | 0.9997× | 1.0× | ✅ |
| wss | 104,464 | 104,794 | 0.9969× | 1.0× | ✅ |
| atomic | 49,684 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,685 | 16,959 | 0.9838× | 0.99× | ✅ |
| http2-concurrent | 359,175 | 408,881 | 0.8784× | 0.88× | ✅ |
| websocket-64k | 156,021 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,808 | 16,327 | 0.7845× | 0.79× | ✅ |
| upload | 478,512 | 1,322,382 | 0.3619× | 0.37× | ✅ |
| static | 29,234 | 40,981 | 0.7133× | 0.72× | ✅ |
| pgsql | 23,187 | 24,594 | 0.9428× | 0.95× | ✅ |
| sqlite | 23,470 | 25,236 | 0.9300× | 0.94× | ✅ |
| curl | 84,738 | 90,483 | 0.9365× | 0.94× | ✅ |
| dns | 230,705 | 232,620 | 0.9918× | 0.99× | ✅ |

Geometric mean 0.8826×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
