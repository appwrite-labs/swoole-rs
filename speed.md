# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d0a1aa4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,949 | 15,213 | 0.9169× | 0.92× | ✅ |
| echo-1k | 17,050 | 18,165 | 0.9386× | 0.94× | ✅ |
| json | 22,116 | 23,158 | 0.9550× | 0.95× | ✅ |
| service-health | 16,557 | 17,773 | 0.9316× | 0.93× | ✅ |
| websocket | 6,636 | 8,188 | 0.8105× | 0.8× | ✅ |
| timer | 205,943 | 313,511 | 0.6569× | 0.65× | ✅ |
| app | 125,551 | 135,876 | 0.9240× | 0.93× | ✅ |
| http2 | 18,675 | 20,464 | 0.9126× | 0.91× | ✅ |
| table | 196,015 | 201,865 | 0.9710× | 0.97× | ✅ |
| channel | 136,708 | 155,170 | 0.8810× | 0.89× | ✅ |
| task | 32,499 | 33,265 | 0.9770× | 0.96× | ✅ |
| mysql | 49,697 | 51,053 | 0.9734× | 0.97× | ✅ |
| http-client | 33,910 | 35,804 | 0.9471× | 0.94× | ✅ |
| client | 18,602 | 19,757 | 0.9415× | 0.94× | ✅ |
| socket | 19,723 | 21,072 | 0.9360× | 0.92× | ✅ |
| gzip | 194,166 | 197,083 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,572 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,765 | 0.9899× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,228 | 0.5094× | 0.51× | ✅ |
| udp | 7,508 | 7,987 | 0.9401× | 0.93× | ✅ |
| connect | 17,704 | 18,707 | 0.9464× | 0.94× | ✅ |
| https | 108,196 | 108,806 | 0.9944× | 1.0× | ✅ |
| wss | 103,859 | 104,797 | 0.9911× | 0.99× | ✅ |
| atomic | 49,710 | 51,316 | 0.9687× | 0.97× | ✅ |
| process-mode | 15,101 | 16,962 | 0.8902× | 0.88× | ✅ |
| http2-concurrent | 361,455 | 408,877 | 0.8840× | 0.88× | ✅ |
| websocket-64k | 89,959 | 159,691 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 12,074 | 16,331 | 0.7393× | 0.74× | ✅ |
| upload | 412,181 | 1,322,388 | 0.3117× | 0.32× | ✅ |
| static | 27,333 | 41,074 | 0.6654× | 0.66× | ✅ |
| pgsql | 23,340 | 24,597 | 0.9489× | 0.95× | ✅ |
| sqlite | 23,547 | 25,247 | 0.9327× | 0.93× | ✅ |
| curl | 85,896 | 90,355 | 0.9507× | 0.94× | ✅ |
| dns | 231,736 | 232,898 | 0.9950× | 0.99× | ✅ |

Geometric mean 0.8582×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
