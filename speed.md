# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `6614c9a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,929 | 15,213 | 0.9156× | 0.92× | ✅ |
| echo-1k | 17,042 | 18,165 | 0.9381× | 0.94× | ✅ |
| json | 22,018 | 23,161 | 0.9506× | 0.96× | ✅ |
| service-health | 16,517 | 17,777 | 0.9291× | 0.93× | ✅ |
| websocket | 6,715 | 8,191 | 0.8198× | 0.82× | ✅ |
| timer | 202,529 | 313,512 | 0.6460× | 0.65× | ✅ |
| app | 125,925 | 135,969 | 0.9261× | 0.93× | ✅ |
| http2 | 19,828 | 20,468 | 0.9687× | 0.97× | ✅ |
| table | 195,725 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,873 | 154,654 | 0.8980× | 0.9× | ✅ |
| task | 31,946 | 33,268 | 0.9603× | 0.96× | ✅ |
| mysql | 49,567 | 51,061 | 0.9707× | 0.97× | ✅ |
| http-client | 33,618 | 35,806 | 0.9389× | 0.94× | ✅ |
| client | 18,468 | 19,757 | 0.9348× | 0.94× | ✅ |
| socket | 19,752 | 21,067 | 0.9375× | 0.92× | ✅ |
| gzip | 196,007 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,477 | 181,572 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,765 | 1.0540× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,231 | 0.5439× | 0.54× | ✅ |
| udp | 8,038 | 7,987 | 1.0064× | 1.0× | ❌ |
| connect | 18,264 | 18,707 | 0.9763× | 0.98× | ✅ |
| https | 108,818 | 108,810 | 1.0001× | 1.01× | ➖ |
| wss | 104,683 | 104,794 | 0.9989× | 1.0× | ✅ |
| atomic | 49,676 | 51,316 | 0.9681× | 0.97× | ✅ |
| process-mode | 16,436 | 17,025 | 0.9654× | 0.97× | ✅ |
| http2-concurrent | 372,965 | 408,877 | 0.9122× | 0.91× | ✅ |
| websocket-64k | 156,163 | 159,697 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,076 | 16,331 | 0.8007× | 0.79× | ✅ |
| upload | 544,641 | 1,322,382 | 0.4119× | 0.42× | ✅ |
| static | 34,449 | 41,045 | 0.8393× | 0.84× | ✅ |
| pgsql | 23,204 | 24,594 | 0.9435× | 0.95× | ✅ |
| sqlite | 23,484 | 25,232 | 0.9307× | 0.94× | ✅ |
| curl | 85,321 | 90,493 | 0.9428× | 0.95× | ✅ |
| dns | 230,939 | 232,479 | 0.9934× | 1.0× | ✅ |

Geometric mean 0.8978×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
