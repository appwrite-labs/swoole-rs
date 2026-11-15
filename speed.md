# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `6cdc1c3` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,951 | 15,213 | 0.9171× | 0.92× | ✅ |
| echo-1k | 17,055 | 18,165 | 0.9389× | 0.94× | ✅ |
| json | 22,119 | 23,161 | 0.9550× | 0.95× | ✅ |
| service-health | 16,556 | 17,777 | 0.9314× | 0.93× | ✅ |
| websocket | 6,618 | 8,188 | 0.8083× | 0.8× | ✅ |
| timer | 205,706 | 313,506 | 0.6561× | 0.65× | ✅ |
| app | 125,304 | 135,882 | 0.9222× | 0.93× | ✅ |
| http2 | 18,734 | 20,468 | 0.9153× | 0.91× | ✅ |
| table | 195,200 | 201,865 | 0.9670× | 0.97× | ✅ |
| channel | 136,956 | 155,174 | 0.8826× | 0.88× | ✅ |
| task | 32,515 | 33,268 | 0.9774× | 0.96× | ✅ |
| mysql | 49,517 | 51,064 | 0.9697× | 0.97× | ✅ |
| http-client | 33,806 | 35,800 | 0.9443× | 0.94× | ✅ |
| client | 18,528 | 19,756 | 0.9378× | 0.94× | ✅ |
| socket | 19,649 | 21,073 | 0.9325× | 0.92× | ✅ |
| gzip | 194,157 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,057 | 181,572 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,707 | 4,762 | 0.9884× | 0.99× | ✅ |
| tcp-eof | 5,202 | 10,231 | 0.5084× | 0.51× | ✅ |
| udp | 7,511 | 7,987 | 0.9405× | 0.93× | ✅ |
| connect | 17,717 | 18,707 | 0.9471× | 0.94× | ✅ |
| https | 108,197 | 108,810 | 0.9944× | 1.0× | ✅ |
| wss | 103,828 | 104,803 | 0.9907× | 0.99× | ✅ |
| atomic | 49,695 | 51,316 | 0.9684× | 0.97× | ✅ |
| process-mode | 15,116 | 17,027 | 0.8878× | 0.88× | ✅ |
| http2-concurrent | 362,458 | 409,069 | 0.8861× | 0.88× | ✅ |
| websocket-64k | 89,949 | 159,692 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,857 | 16,331 | 0.7260× | 0.73× | ✅ |
| upload | 413,508 | 1,322,391 | 0.3127× | 0.32× | ✅ |
| static | 27,283 | 40,987 | 0.6657× | 0.66× | ✅ |
| pgsql | 23,369 | 24,594 | 0.9502× | 0.95× | ✅ |
| sqlite | 23,545 | 25,236 | 0.9330× | 0.93× | ✅ |
| curl | 85,935 | 90,354 | 0.9511× | 0.94× | ✅ |
| dns | 231,885 | 233,031 | 0.9951× | 0.99× | ✅ |

Geometric mean 0.8573×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
