# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `571cebd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,204 | 15,214 | 0.9336× | 0.93× | ✅ |
| echo-1k | 17,322 | 18,165 | 0.9536× | 0.95× | ✅ |
| json | 22,307 | 23,161 | 0.9631× | 0.96× | ✅ |
| service-health | 16,805 | 17,777 | 0.9453× | 0.94× | ✅ |
| websocket | 6,748 | 8,191 | 0.8238× | 0.82× | ✅ |
| timer | 202,144 | 313,493 | 0.6448× | 0.65× | ✅ |
| app | 126,371 | 135,957 | 0.9295× | 0.93× | ✅ |
| http2 | 19,988 | 20,464 | 0.9767× | 0.98× | ✅ |
| table | 196,021 | 201,869 | 0.9710× | 0.97× | ✅ |
| channel | 138,448 | 154,651 | 0.8952× | 0.91× | ✅ |
| task | 32,202 | 33,261 | 0.9682× | 0.97× | ✅ |
| mysql | 49,854 | 51,064 | 0.9763× | 0.98× | ✅ |
| http-client | 33,815 | 35,808 | 0.9443× | 0.95× | ✅ |
| client | 18,765 | 19,757 | 0.9498× | 0.95× | ✅ |
| socket | 19,731 | 21,067 | 0.9366× | 0.94× | ✅ |
| gzip | 196,175 | 197,079 | 0.9954× | 1.0× | ✅ |
| websocket-deflate | 181,503 | 181,568 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,238 | 4,762 | 1.0999× | 1.09× | ❌ |
| tcp-eof | 5,769 | 10,231 | 0.5638× | 0.56× | ✅ |
| udp | 7,997 | 7,986 | 1.0013× | 1.0× | ❌ |
| connect | 18,429 | 18,707 | 0.9851× | 0.99× | ✅ |
| https | 109,152 | 108,813 | 1.0031× | 1.01× | ➖ |
| wss | 104,693 | 104,800 | 0.9990× | 1.0× | ✅ |
| atomic | 49,972 | 51,316 | 0.9738× | 0.98× | ✅ |
| process-mode | 16,623 | 16,963 | 0.9800× | 0.98× | ✅ |
| http2-concurrent | 375,601 | 408,882 | 0.9186× | 0.92× | ✅ |
| websocket-64k | 156,199 | 159,694 | 0.9781× | 0.98× | ✅ |
| websocket-fragmented | 13,012 | 16,328 | 0.7969× | 0.79× | ✅ |
| upload | 544,420 | 1,322,385 | 0.4117× | 0.42× | ✅ |
| static | 34,308 | 40,981 | 0.8372× | 0.84× | ✅ |
| pgsql | 23,603 | 24,594 | 0.9597× | 0.96× | ✅ |
| sqlite | 23,838 | 25,232 | 0.9448× | 0.95× | ✅ |
| curl | 85,646 | 90,410 | 0.9473× | 0.95× | ✅ |
| dns | 230,299 | 232,600 | 0.9901× | 1.0× | ✅ |

Geometric mean 0.9045×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
