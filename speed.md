# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `376c96d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,853 | 15,213 | 0.9106× | 0.91× | ✅ |
| echo-1k | 16,920 | 18,165 | 0.9314× | 0.93× | ✅ |
| json | 22,073 | 23,162 | 0.9530× | 0.95× | ✅ |
| service-health | 16,447 | 17,777 | 0.9252× | 0.92× | ✅ |
| websocket | 6,517 | 8,188 | 0.7959× | 0.8× | ✅ |
| timer | 206,473 | 313,517 | 0.6586× | 0.65× | ✅ |
| app | 126,241 | 135,968 | 0.9285× | 0.93× | ✅ |
| http2 | 19,068 | 20,468 | 0.9316× | 0.92× | ✅ |
| table | 194,886 | 201,865 | 0.9654× | 0.97× | ✅ |
| channel | 137,649 | 155,170 | 0.8871× | 0.88× | ✅ |
| task | 32,565 | 33,268 | 0.9789× | 0.96× | ✅ |
| mysql | 49,716 | 51,062 | 0.9736× | 0.97× | ✅ |
| http-client | 33,494 | 35,800 | 0.9356× | 0.93× | ✅ |
| gzip | 193,999 | 197,083 | 0.9844× | 0.99× | ✅ |
| websocket-deflate | 178,897 | 181,572 | 0.9853× | 0.99× | ✅ |
| tcp-length | 4,545 | 4,765 | 0.9537× | 0.96× | ✅ |
| tcp-eof | 5,125 | 10,231 | 0.5009× | 0.5× | ✅ |
| udp | 7,453 | 7,987 | 0.9332× | 0.93× | ✅ |
| connect | 17,499 | 18,703 | 0.9356× | 0.93× | ✅ |
| https | 106,005 | 108,813 | 0.9742× | 0.98× | ✅ |
| wss | 100,933 | 104,797 | 0.9631× | 0.97× | ✅ |
| atomic | 49,924 | 51,316 | 0.9729× | 0.97× | ✅ |
| process-mode | 14,994 | 17,025 | 0.8807× | 0.88× | ✅ |
| http2-concurrent | 365,408 | 409,246 | 0.8929× | 0.88× | ✅ |
| websocket-64k | 89,774 | 159,694 | 0.5622× | 0.57× | ✅ |
| websocket-fragmented | 11,578 | 16,331 | 0.7090× | 0.72× | ✅ |
| upload | 413,796 | 1,322,387 | 0.3129× | 0.32× | ✅ |
| static | 26,923 | 41,010 | 0.6565× | 0.65× | ✅ |
| client | 18,507 | 19,757 | 0.9367× | 0.94× | ✅ |
| socket | 19,670 | 21,073 | 0.9334× | 0.92× | ✅ |
| pgsql | 22,972 | 24,595 | 0.9340× | 0.93× | ✅ |
| sqlite | 23,490 | 25,221 | 0.9313× | 0.93× | ✅ |
| curl | 85,094 | 90,354 | 0.9418× | 0.94× | ✅ |
| dns | 230,368 | 232,847 | 0.9894× | 0.99× | ✅ |

Geometric mean 0.8523×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
