# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9aea4c6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,941 | 15,216 | 0.9162× | 0.92× | ✅ |
| echo-1k | 17,042 | 18,161 | 0.9384× | 0.94× | ✅ |
| json | 22,036 | 23,158 | 0.9515× | 0.95× | ✅ |
| service-health | 16,542 | 17,771 | 0.9308× | 0.93× | ✅ |
| websocket | 6,631 | 8,188 | 0.8099× | 0.82× | ✅ |
| timer | 203,281 | 313,531 | 0.6484× | 0.65× | ✅ |
| app | 125,903 | 135,894 | 0.9265× | 0.93× | ✅ |
| http2 | 18,694 | 20,485 | 0.9125× | 0.93× | ✅ |
| table | 195,297 | 201,916 | 0.9672× | 0.97× | ✅ |
| channel | 138,893 | 154,724 | 0.8977× | 0.9× | ✅ |
| task | 32,332 | 33,307 | 0.9707× | 0.96× | ✅ |
| mysql | 49,600 | 51,050 | 0.9716× | 0.97× | ✅ |
| http-client | 33,748 | 35,818 | 0.9422× | 0.94× | ✅ |
| client | 18,495 | 19,757 | 0.9361× | 0.94× | ✅ |
| socket | 19,614 | 21,062 | 0.9312× | 0.92× | ✅ |
| gzip | 196,017 | 197,084 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,395 | 181,576 | 0.9990× | 1.0× | ✅ |
| tcp-length | 4,839 | 4,762 | 1.0161× | 1.05× | ➖ |
| tcp-eof | 5,387 | 10,231 | 0.5265× | 0.54× | ✅ |
| udp | 7,623 | 7,987 | 0.9544× | 0.95× | ✅ |
| connect | 18,428 | 18,710 | 0.9849× | 0.98× | ✅ |
| https | 108,801 | 108,803 | 1.0000× | 1.0× | ✅ |
| wss | 104,515 | 104,797 | 0.9973× | 1.0× | ✅ |
| atomic | 49,696 | 51,316 | 0.9684× | 0.97× | ✅ |
| process-mode | 16,771 | 16,963 | 0.9887× | 0.99× | ✅ |
| http2-concurrent | 359,396 | 408,781 | 0.8792× | 0.88× | ✅ |
| websocket-64k | 156,078 | 159,695 | 0.9774× | 0.98× | ✅ |
| websocket-fragmented | 12,866 | 16,331 | 0.7878× | 0.79× | ✅ |
| upload | 478,447 | 1,322,386 | 0.3618× | 0.37× | ✅ |
| static | 29,165 | 40,981 | 0.7117× | 0.72× | ✅ |
| pgsql | 23,205 | 24,594 | 0.9435× | 0.95× | ✅ |
| sqlite | 23,484 | 25,217 | 0.9313× | 0.94× | ✅ |
| curl | 84,714 | 90,364 | 0.9375× | 0.94× | ✅ |
| dns | 230,267 | 232,602 | 0.9900× | 0.99× | ✅ |

Geometric mean 0.8845×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
