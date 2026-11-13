# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `86f98ee` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,950 | 15,213 | 0.9170× | 0.92× | ✅ |
| echo-1k | 17,074 | 18,165 | 0.9399× | 0.94× | ✅ |
| json | 22,066 | 23,161 | 0.9527× | 0.95× | ✅ |
| service-health | 16,568 | 17,777 | 0.9320× | 0.93× | ✅ |
| websocket | 6,559 | 8,188 | 0.8011× | 0.81× | ✅ |
| timer | 203,362 | 314,298 | 0.6470× | 0.65× | ✅ |
| app | 125,442 | 135,943 | 0.9228× | 0.93× | ✅ |
| http2 | 18,621 | 20,467 | 0.9098× | 0.92× | ✅ |
| table | 195,546 | 201,865 | 0.9687× | 0.97× | ✅ |
| channel | 137,483 | 154,654 | 0.8890× | 0.89× | ✅ |
| task | 32,021 | 33,264 | 0.9626× | 0.96× | ✅ |
| mysql | 49,564 | 51,040 | 0.9711× | 0.97× | ✅ |
| http-client | 33,773 | 35,805 | 0.9432× | 0.94× | ✅ |
| client | 18,523 | 19,757 | 0.9375× | 0.94× | ✅ |
| socket | 19,647 | 21,067 | 0.9326× | 0.92× | ✅ |
| gzip | 194,151 | 197,083 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,349 | 181,572 | 0.9878× | 0.99× | ✅ |
| tcp-length | 4,774 | 4,762 | 1.0026× | 1.01× | ➖ |
| tcp-eof | 5,297 | 10,231 | 0.5178× | 0.52× | ✅ |
| udp | 7,564 | 7,986 | 0.9471× | 0.95× | ✅ |
| connect | 18,069 | 18,707 | 0.9659× | 0.98× | ✅ |
| https | 108,724 | 108,806 | 0.9992× | 1.0× | ✅ |
| wss | 104,294 | 104,803 | 0.9951× | 1.0× | ✅ |
| atomic | 49,752 | 51,316 | 0.9695× | 0.97× | ✅ |
| process-mode | 15,758 | 16,963 | 0.9290× | 0.94× | ✅ |
| http2-concurrent | 357,744 | 408,877 | 0.8749× | 0.88× | ✅ |
| websocket-64k | 156,011 | 159,691 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,789 | 16,331 | 0.7831× | 0.79× | ✅ |
| upload | 477,925 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 28,790 | 41,045 | 0.7014× | 0.72× | ✅ |
| pgsql | 23,263 | 24,595 | 0.9458× | 0.95× | ✅ |
| sqlite | 23,526 | 25,236 | 0.9322× | 0.94× | ✅ |
| curl | 85,773 | 90,343 | 0.9494× | 0.94× | ✅ |
| dns | 231,287 | 233,123 | 0.9921× | 0.99× | ✅ |

Geometric mean 0.8799×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
