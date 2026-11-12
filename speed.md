# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `708fbd8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,918 | 15,210 | 0.9151× | 0.92× | ✅ |
| echo-1k | 17,021 | 18,165 | 0.9370× | 0.94× | ✅ |
| json | 22,006 | 23,161 | 0.9501× | 0.95× | ✅ |
| service-health | 16,501 | 17,773 | 0.9284× | 0.93× | ✅ |
| websocket | 6,628 | 8,191 | 0.8091× | 0.82× | ✅ |
| timer | 203,266 | 313,566 | 0.6482× | 0.65× | ✅ |
| app | 125,824 | 135,971 | 0.9254× | 0.93× | ✅ |
| http2 | 18,637 | 20,465 | 0.9107× | 0.93× | ✅ |
| table | 195,711 | 201,865 | 0.9695× | 0.97× | ✅ |
| channel | 138,859 | 154,651 | 0.8979× | 0.9× | ✅ |
| task | 32,283 | 33,268 | 0.9704× | 0.96× | ✅ |
| mysql | 49,563 | 51,038 | 0.9711× | 0.97× | ✅ |
| http-client | 33,653 | 35,809 | 0.9398× | 0.94× | ✅ |
| client | 18,461 | 19,757 | 0.9344× | 0.94× | ✅ |
| socket | 19,579 | 21,067 | 0.9294× | 0.92× | ✅ |
| gzip | 196,004 | 197,083 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,394 | 181,572 | 0.9990× | 1.0× | ✅ |
| tcp-length | 4,839 | 4,762 | 1.0161× | 1.05× | ➖ |
| tcp-eof | 5,387 | 10,231 | 0.5265× | 0.54× | ✅ |
| udp | 7,619 | 7,987 | 0.9539× | 0.95× | ✅ |
| connect | 18,404 | 18,703 | 0.9840× | 0.98× | ✅ |
| https | 108,776 | 108,810 | 0.9997× | 1.0× | ✅ |
| wss | 104,532 | 104,800 | 0.9974× | 1.0× | ✅ |
| atomic | 49,662 | 51,316 | 0.9678× | 0.97× | ✅ |
| process-mode | 16,747 | 16,963 | 0.9873× | 0.97× | ✅ |
| http2-concurrent | 359,459 | 409,279 | 0.8783× | 0.88× | ✅ |
| websocket-64k | 156,072 | 159,703 | 0.9773× | 0.98× | ✅ |
| websocket-fragmented | 12,862 | 16,331 | 0.7876× | 0.79× | ✅ |
| upload | 478,418 | 1,322,380 | 0.3618× | 0.37× | ✅ |
| static | 29,181 | 40,981 | 0.7121× | 0.72× | ✅ |
| pgsql | 23,208 | 24,594 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,488 | 25,236 | 0.9308× | 0.94× | ✅ |
| curl | 84,755 | 90,378 | 0.9378× | 0.94× | ✅ |
| dns | 230,572 | 233,266 | 0.9885× | 0.99× | ✅ |

Geometric mean 0.8840×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
