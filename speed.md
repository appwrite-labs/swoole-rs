# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d32eb59` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,207 | 15,214 | 0.9338× | 0.93× | ✅ |
| echo-1k | 17,322 | 18,165 | 0.9536× | 0.95× | ✅ |
| json | 22,307 | 23,161 | 0.9631× | 0.96× | ✅ |
| service-health | 16,805 | 17,777 | 0.9453× | 0.94× | ✅ |
| websocket | 6,748 | 8,191 | 0.8238× | 0.82× | ✅ |
| timer | 202,132 | 313,820 | 0.6441× | 0.65× | ✅ |
| app | 126,347 | 135,944 | 0.9294× | 0.93× | ✅ |
| http2 | 19,991 | 20,463 | 0.9769× | 0.98× | ✅ |
| table | 196,021 | 201,865 | 0.9710× | 0.97× | ✅ |
| channel | 138,448 | 154,651 | 0.8952× | 0.91× | ✅ |
| task | 32,202 | 33,264 | 0.9681× | 0.97× | ✅ |
| mysql | 49,854 | 51,059 | 0.9764× | 0.98× | ✅ |
| http-client | 33,818 | 35,806 | 0.9445× | 0.95× | ✅ |
| client | 18,761 | 19,757 | 0.9496× | 0.95× | ✅ |
| socket | 19,731 | 21,067 | 0.9366× | 0.94× | ✅ |
| gzip | 196,175 | 197,079 | 0.9954× | 1.0× | ✅ |
| websocket-deflate | 181,503 | 181,568 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,238 | 4,765 | 1.0991× | 1.09× | ❌ |
| tcp-eof | 5,769 | 10,231 | 0.5638× | 0.56× | ✅ |
| udp | 7,997 | 7,986 | 1.0013× | 1.0× | ❌ |
| connect | 18,428 | 18,707 | 0.9851× | 0.99× | ✅ |
| https | 109,133 | 108,810 | 1.0030× | 1.01× | ➖ |
| wss | 104,693 | 104,800 | 0.9990× | 1.0× | ✅ |
| atomic | 49,968 | 51,316 | 0.9737× | 0.98× | ✅ |
| process-mode | 16,623 | 16,959 | 0.9801× | 0.98× | ✅ |
| http2-concurrent | 375,757 | 408,877 | 0.9190× | 0.92× | ✅ |
| websocket-64k | 156,194 | 159,693 | 0.9781× | 0.98× | ✅ |
| websocket-fragmented | 13,007 | 16,331 | 0.7965× | 0.79× | ✅ |
| upload | 544,406 | 1,322,382 | 0.4117× | 0.42× | ✅ |
| static | 34,298 | 40,981 | 0.8369× | 0.84× | ✅ |
| pgsql | 23,605 | 24,594 | 0.9598× | 0.96× | ✅ |
| sqlite | 23,838 | 25,232 | 0.9448× | 0.95× | ✅ |
| curl | 85,642 | 90,484 | 0.9465× | 0.95× | ✅ |
| dns | 231,252 | 233,128 | 0.9920× | 1.0× | ✅ |

Geometric mean 0.9045×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
