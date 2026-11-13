# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b297d3e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,870 | 15,213 | 0.9117× | 0.92× | ✅ |
| echo-1k | 16,970 | 18,162 | 0.9344× | 0.94× | ✅ |
| json | 21,962 | 23,158 | 0.9484× | 0.95× | ✅ |
| service-health | 16,454 | 17,777 | 0.9256× | 0.93× | ✅ |
| websocket | 6,479 | 8,188 | 0.7913× | 0.8× | ✅ |
| timer | 203,039 | 313,512 | 0.6476× | 0.65× | ✅ |
| app | 125,335 | 135,945 | 0.9220× | 0.93× | ✅ |
| http2 | 18,479 | 20,464 | 0.9030× | 0.91× | ✅ |
| table | 195,428 | 201,865 | 0.9681× | 0.97× | ✅ |
| channel | 137,369 | 155,167 | 0.8853× | 0.89× | ✅ |
| task | 31,924 | 33,268 | 0.9596× | 0.96× | ✅ |
| mysql | 49,466 | 51,064 | 0.9687× | 0.97× | ✅ |
| http-client | 33,629 | 35,797 | 0.9395× | 0.94× | ✅ |
| client | 18,405 | 19,757 | 0.9316× | 0.94× | ✅ |
| socket | 19,533 | 21,072 | 0.9270× | 0.92× | ✅ |
| gzip | 194,066 | 197,079 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,306 | 181,568 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,763 | 0.9847× | 0.99× | ✅ |
| tcp-eof | 5,213 | 10,228 | 0.5097× | 0.51× | ✅ |
| udp | 7,378 | 7,986 | 0.9238× | 0.93× | ✅ |
| connect | 17,539 | 18,707 | 0.9376× | 0.95× | ✅ |
| https | 108,028 | 108,810 | 0.9928× | 1.0× | ✅ |
| wss | 103,623 | 104,801 | 0.9888× | 0.99× | ✅ |
| atomic | 49,571 | 51,316 | 0.9660× | 0.97× | ✅ |
| process-mode | 14,896 | 16,960 | 0.8783× | 0.89× | ✅ |
| http2-concurrent | 357,537 | 409,073 | 0.8740× | 0.88× | ✅ |
| websocket-64k | 155,926 | 159,693 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,704 | 16,327 | 0.7781× | 0.78× | ✅ |
| upload | 477,825 | 1,322,388 | 0.3613× | 0.37× | ✅ |
| static | 27,266 | 40,987 | 0.6652× | 0.67× | ✅ |
| pgsql | 23,158 | 24,594 | 0.9416× | 0.95× | ✅ |
| sqlite | 23,455 | 25,236 | 0.9294× | 0.93× | ✅ |
| curl | 85,648 | 90,477 | 0.9466× | 0.94× | ✅ |
| dns | 231,076 | 232,655 | 0.9932× | 0.99× | ✅ |

Geometric mean 0.8721×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
