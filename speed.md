# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8b7e300` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,754 | 15,210 | 0.9043× | 0.91× | ✅ |
| echo-1k | 16,733 | 18,165 | 0.9212× | 0.93× | ✅ |
| json | 22,058 | 23,161 | 0.9524× | 0.95× | ✅ |
| service-health | 16,334 | 17,773 | 0.9190× | 0.92× | ✅ |
| websocket | 6,625 | 8,191 | 0.8088× | 0.8× | ✅ |
| timer | 202,000 | 313,826 | 0.6437× | 0.65× | ✅ |
| app | 126,363 | 135,957 | 0.9294× | 0.93× | ✅ |
| http2 | 19,092 | 20,498 | 0.9314× | 0.92× | ✅ |
| table | 193,861 | 201,876 | 0.9603× | 0.96× | ✅ |
| channel | 139,208 | 155,717 | 0.8940× | 0.88× | ✅ |
| task | 31,952 | 33,268 | 0.9604× | 0.96× | ✅ |
| mysql | 49,761 | 51,061 | 0.9745× | 0.97× | ✅ |
| http-client | 33,459 | 35,803 | 0.9345× | 0.93× | ✅ |
| gzip | 194,205 | 197,093 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,043 | 181,569 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,599 | 4,765 | 0.9651× | 0.96× | ✅ |
| tcp-eof | 5,097 | 10,228 | 0.4983× | 0.5× | ✅ |
| udp | 7,545 | 7,986 | 0.9448× | 0.95× | ✅ |
| connect | 17,292 | 18,686 | 0.9254× | 0.93× | ✅ |
| https | 106,030 | 108,806 | 0.9745× | 0.98× | ✅ |
| wss | 101,171 | 104,794 | 0.9654× | 0.97× | ✅ |
| atomic | 49,808 | 51,272 | 0.9714× | 0.97× | ✅ |
| process-mode | 15,236 | 17,024 | 0.8950× | 0.89× | ✅ |
| http2-concurrent | 366,779 | 409,074 | 0.8966× | 0.9× | ✅ |
| websocket-64k | 89,910 | 159,691 | 0.5630× | 0.57× | ✅ |
| websocket-fragmented | 11,758 | 16,368 | 0.7184× | 0.72× | ✅ |
| upload | 415,971 | 1,322,413 | 0.3146× | 0.32× | ✅ |
| static | 26,760 | 40,960 | 0.6533× | 0.65× | ✅ |
| client | 18,389 | 19,757 | 0.9308× | 0.94× | ✅ |
| socket | 19,549 | 21,072 | 0.9277× | 0.93× | ✅ |
| pgsql | 22,618 | 24,594 | 0.9196× | 0.93× | ✅ |
| sqlite | 23,437 | 25,236 | 0.9287× | 0.93× | ✅ |
| curl | 86,116 | 90,351 | 0.9531× | 0.95× | ✅ |
| dns | 230,325 | 232,620 | 0.9901× | 0.99× | ✅ |

Geometric mean 0.8518×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
