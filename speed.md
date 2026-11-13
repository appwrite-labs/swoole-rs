# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `03ddb33` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,916 | 15,210 | 0.9149× | 0.92× | ✅ |
| echo-1k | 17,018 | 18,162 | 0.9370× | 0.94× | ✅ |
| json | 22,013 | 23,161 | 0.9504× | 0.95× | ✅ |
| service-health | 16,515 | 17,773 | 0.9292× | 0.93× | ✅ |
| websocket | 6,573 | 8,191 | 0.8025× | 0.82× | ✅ |
| timer | 203,177 | 313,714 | 0.6477× | 0.65× | ✅ |
| app | 125,748 | 135,855 | 0.9256× | 0.93× | ✅ |
| http2 | 18,649 | 20,464 | 0.9113× | 0.93× | ✅ |
| table | 195,468 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 137,398 | 154,651 | 0.8884× | 0.9× | ✅ |
| task | 32,151 | 33,268 | 0.9664× | 0.96× | ✅ |
| mysql | 49,509 | 51,047 | 0.9699× | 0.97× | ✅ |
| http-client | 33,716 | 35,805 | 0.9417× | 0.94× | ✅ |
| client | 18,470 | 19,757 | 0.9349× | 0.94× | ✅ |
| socket | 19,586 | 21,067 | 0.9297× | 0.92× | ✅ |
| gzip | 194,117 | 197,083 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,379 | 181,572 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,788 | 4,762 | 1.0054× | 1.05× | ➖ |
| tcp-eof | 5,336 | 10,228 | 0.5217× | 0.54× | ✅ |
| udp | 7,577 | 7,986 | 0.9487× | 0.95× | ✅ |
| connect | 18,349 | 18,707 | 0.9809× | 0.98× | ✅ |
| https | 108,783 | 108,810 | 0.9998× | 1.0× | ✅ |
| wss | 104,482 | 104,800 | 0.9970× | 1.0× | ✅ |
| atomic | 49,675 | 51,316 | 0.9680× | 0.97× | ✅ |
| process-mode | 16,664 | 16,960 | 0.9826× | 0.99× | ✅ |
| http2-concurrent | 358,911 | 408,877 | 0.8778× | 0.88× | ✅ |
| websocket-64k | 156,019 | 159,696 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,808 | 16,331 | 0.7843× | 0.79× | ✅ |
| upload | 478,490 | 1,322,382 | 0.3618× | 0.37× | ✅ |
| static | 29,339 | 41,132 | 0.7133× | 0.72× | ✅ |
| pgsql | 23,211 | 24,596 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,489 | 25,236 | 0.9308× | 0.94× | ✅ |
| curl | 84,764 | 90,248 | 0.9392× | 0.94× | ✅ |
| dns | 230,353 | 232,507 | 0.9907× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
