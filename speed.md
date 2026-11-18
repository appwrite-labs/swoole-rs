# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `926cb2e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,742 | 15,213 | 0.9033× | 0.91× | ✅ |
| echo-1k | 16,738 | 18,165 | 0.9214× | 0.92× | ✅ |
| json | 22,070 | 23,164 | 0.9528× | 0.95× | ✅ |
| service-health | 16,314 | 17,777 | 0.9177× | 0.92× | ✅ |
| websocket | 6,681 | 8,189 | 0.8158× | 0.81× | ✅ |
| timer | 201,548 | 314,031 | 0.6418× | 0.65× | ✅ |
| app | 127,075 | 135,896 | 0.9351× | 0.93× | ✅ |
| http2 | 18,974 | 20,496 | 0.9257× | 0.92× | ✅ |
| table | 193,458 | 202,900 | 0.9535× | 0.96× | ✅ |
| channel | 137,972 | 155,717 | 0.8860× | 0.88× | ✅ |
| task | 31,945 | 33,272 | 0.9601× | 0.96× | ✅ |
| mysql | 50,064 | 51,085 | 0.9800× | 0.97× | ✅ |
| http-client | 33,422 | 35,792 | 0.9338× | 0.93× | ✅ |
| gzip | 194,360 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,763 | 0.9750× | 0.96× | ✅ |
| tcp-eof | 5,201 | 10,231 | 0.5083× | 0.5× | ✅ |
| udp | 7,580 | 7,987 | 0.9490× | 0.95× | ✅ |
| connect | 17,580 | 18,734 | 0.9384× | 0.93× | ✅ |
| https | 106,026 | 108,802 | 0.9745× | 0.98× | ✅ |
| wss | 101,243 | 104,797 | 0.9661× | 0.97× | ✅ |
| atomic | 50,173 | 51,276 | 0.9785× | 0.97× | ✅ |
| process-mode | 15,270 | 17,028 | 0.8968× | 0.89× | ✅ |
| http2-concurrent | 364,038 | 408,861 | 0.8904× | 0.89× | ✅ |
| websocket-64k | 89,976 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,840 | 16,368 | 0.7234× | 0.72× | ✅ |
| upload | 416,448 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,041 | 41,028 | 0.6591× | 0.65× | ✅ |
| client | 18,478 | 19,721 | 0.9370× | 0.93× | ✅ |
| socket | 19,704 | 21,032 | 0.9368× | 0.93× | ✅ |
| pgsql | 22,920 | 24,618 | 0.9310× | 0.92× | ✅ |
| sqlite | 23,406 | 25,236 | 0.9275× | 0.93× | ✅ |
| curl | 85,360 | 90,356 | 0.9447× | 0.95× | ✅ |
| dns | 229,920 | 232,869 | 0.9873× | 0.99× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
