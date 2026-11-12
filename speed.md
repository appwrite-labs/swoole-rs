# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c71160c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,916 | 15,213 | 0.9147× | 0.92× | ✅ |
| echo-1k | 17,025 | 18,162 | 0.9374× | 0.94× | ✅ |
| json | 22,013 | 23,161 | 0.9504× | 0.95× | ✅ |
| service-health | 16,515 | 17,777 | 0.9290× | 0.93× | ✅ |
| websocket | 6,576 | 8,191 | 0.8028× | 0.82× | ✅ |
| timer | 203,257 | 313,539 | 0.6483× | 0.65× | ✅ |
| app | 125,723 | 135,844 | 0.9255× | 0.93× | ✅ |
| http2 | 18,668 | 20,464 | 0.9122× | 0.93× | ✅ |
| table | 195,468 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 137,395 | 154,654 | 0.8884× | 0.9× | ✅ |
| task | 32,156 | 33,264 | 0.9667× | 0.96× | ✅ |
| mysql | 49,566 | 51,058 | 0.9708× | 0.97× | ✅ |
| http-client | 33,664 | 35,808 | 0.9401× | 0.94× | ✅ |
| client | 18,473 | 19,758 | 0.9350× | 0.94× | ✅ |
| socket | 19,586 | 21,067 | 0.9297× | 0.92× | ✅ |
| gzip | 194,117 | 197,083 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,379 | 181,572 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,788 | 4,762 | 1.0054× | 1.05× | ➖ |
| tcp-eof | 5,336 | 10,228 | 0.5217× | 0.54× | ✅ |
| udp | 7,577 | 7,986 | 0.9487× | 0.95× | ✅ |
| connect | 18,352 | 18,707 | 0.9811× | 0.98× | ✅ |
| https | 108,792 | 108,813 | 0.9998× | 1.0× | ✅ |
| wss | 104,465 | 104,797 | 0.9968× | 1.0× | ✅ |
| atomic | 49,675 | 51,316 | 0.9680× | 0.97× | ✅ |
| process-mode | 16,671 | 16,959 | 0.9830× | 0.99× | ✅ |
| http2-concurrent | 359,218 | 408,880 | 0.8785× | 0.88× | ✅ |
| websocket-64k | 156,021 | 159,691 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,808 | 16,327 | 0.7845× | 0.79× | ✅ |
| upload | 478,500 | 1,322,381 | 0.3618× | 0.37× | ✅ |
| static | 29,200 | 40,981 | 0.7125× | 0.72× | ✅ |
| pgsql | 23,204 | 24,595 | 0.9434× | 0.95× | ✅ |
| sqlite | 23,487 | 25,236 | 0.9307× | 0.94× | ✅ |
| curl | 84,756 | 90,479 | 0.9367× | 0.94× | ✅ |
| dns | 231,026 | 232,784 | 0.9924× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
