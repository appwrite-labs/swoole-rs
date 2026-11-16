# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b5ac4f7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,847 | 15,213 | 0.9102× | 0.91× | ✅ |
| echo-1k | 16,879 | 18,166 | 0.9292× | 0.93× | ✅ |
| json | 21,999 | 23,161 | 0.9498× | 0.95× | ✅ |
| service-health | 16,440 | 17,777 | 0.9248× | 0.93× | ✅ |
| websocket | 6,509 | 8,188 | 0.7949× | 0.8× | ✅ |
| timer | 206,278 | 313,668 | 0.6576× | 0.65× | ✅ |
| app | 125,995 | 135,908 | 0.9271× | 0.93× | ✅ |
| http2 | 18,835 | 20,468 | 0.9202× | 0.92× | ✅ |
| table | 194,269 | 201,865 | 0.9624× | 0.97× | ✅ |
| channel | 137,906 | 155,174 | 0.8887× | 0.88× | ✅ |
| task | 32,507 | 33,255 | 0.9775× | 0.96× | ✅ |
| mysql | 49,631 | 51,058 | 0.9721× | 0.97× | ✅ |
| http-client | 33,547 | 35,803 | 0.9370× | 0.93× | ✅ |
| gzip | 193,962 | 197,079 | 0.9842× | 0.99× | ✅ |
| websocket-deflate | 178,880 | 181,568 | 0.9852× | 0.99× | ✅ |
| tcp-length | 4,529 | 4,762 | 0.9510× | 0.96× | ✅ |
| tcp-eof | 5,117 | 10,231 | 0.5001× | 0.5× | ✅ |
| udp | 7,414 | 7,986 | 0.9284× | 0.93× | ✅ |
| connect | 17,393 | 18,707 | 0.9297× | 0.93× | ✅ |
| https | 105,890 | 108,810 | 0.9732× | 0.98× | ✅ |
| wss | 100,768 | 104,794 | 0.9616× | 0.97× | ✅ |
| atomic | 49,949 | 51,312 | 0.9734× | 0.97× | ✅ |
| process-mode | 15,019 | 16,959 | 0.8856× | 0.88× | ✅ |
| http2-concurrent | 362,861 | 409,073 | 0.8870× | 0.88× | ✅ |
| websocket-64k | 89,762 | 159,696 | 0.5621× | 0.57× | ✅ |
| websocket-fragmented | 11,680 | 16,327 | 0.7154× | 0.72× | ✅ |
| upload | 413,981 | 1,322,389 | 0.3131× | 0.32× | ✅ |
| static | 26,906 | 40,987 | 0.6564× | 0.65× | ✅ |
| client | 18,515 | 19,757 | 0.9371× | 0.94× | ✅ |
| socket | 19,674 | 21,072 | 0.9337× | 0.92× | ✅ |
| pgsql | 22,779 | 24,594 | 0.9262× | 0.93× | ✅ |
| sqlite | 23,488 | 25,221 | 0.9313× | 0.93× | ✅ |
| curl | 84,860 | 90,355 | 0.9392× | 0.94× | ✅ |
| dns | 229,760 | 232,975 | 0.9862× | 0.99× | ✅ |

Geometric mean 0.8510×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
