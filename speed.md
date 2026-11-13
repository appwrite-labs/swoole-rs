# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `328365a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,961 | 15,214 | 0.9177× | 0.92× | ✅ |
| echo-1k | 17,066 | 18,165 | 0.9395× | 0.94× | ✅ |
| json | 22,061 | 23,158 | 0.9526× | 0.95× | ✅ |
| service-health | 16,563 | 17,773 | 0.9319× | 0.93× | ✅ |
| websocket | 6,584 | 8,188 | 0.8041× | 0.82× | ✅ |
| timer | 203,661 | 313,525 | 0.6496× | 0.65× | ✅ |
| app | 125,810 | 135,933 | 0.9255× | 0.93× | ✅ |
| http2 | 18,656 | 20,464 | 0.9116× | 0.93× | ✅ |
| table | 195,475 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 138,439 | 154,654 | 0.8951× | 0.9× | ✅ |
| task | 32,142 | 33,261 | 0.9664× | 0.96× | ✅ |
| mysql | 49,637 | 51,062 | 0.9721× | 0.97× | ✅ |
| http-client | 33,748 | 35,806 | 0.9425× | 0.94× | ✅ |
| client | 18,477 | 19,757 | 0.9352× | 0.94× | ✅ |
| socket | 19,615 | 21,067 | 0.9311× | 0.92× | ✅ |
| gzip | 194,161 | 197,079 | 0.9852× | 1.0× | ✅ |
| websocket-deflate | 179,397 | 181,572 | 0.9880× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,762 | 1.0063× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,231 | 0.5219× | 0.54× | ✅ |
| udp | 7,573 | 7,987 | 0.9481× | 0.95× | ✅ |
| connect | 18,374 | 18,707 | 0.9822× | 0.98× | ✅ |
| https | 108,795 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,495 | 104,794 | 0.9971× | 1.0× | ✅ |
| atomic | 49,682 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,683 | 16,959 | 0.9837× | 0.99× | ✅ |
| http2-concurrent | 358,766 | 408,881 | 0.8774× | 0.88× | ✅ |
| websocket-64k | 156,024 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,829 | 16,327 | 0.7857× | 0.79× | ✅ |
| upload | 477,967 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 29,307 | 41,074 | 0.7135× | 0.72× | ✅ |
| pgsql | 23,186 | 24,595 | 0.9427× | 0.95× | ✅ |
| sqlite | 23,470 | 25,236 | 0.9300× | 0.94× | ✅ |
| curl | 84,736 | 90,354 | 0.9378× | 0.94× | ✅ |
| dns | 230,638 | 232,648 | 0.9914× | 0.99× | ✅ |

Geometric mean 0.8828×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
