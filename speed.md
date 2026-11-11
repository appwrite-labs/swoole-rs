# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a5f47df` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,204 | 15,214 | 0.9336× | 0.93× | ✅ |
| echo-1k | 17,326 | 18,165 | 0.9538× | 0.95× | ✅ |
| json | 22,312 | 23,162 | 0.9633× | 0.96× | ✅ |
| service-health | 16,810 | 17,777 | 0.9456× | 0.94× | ✅ |
| websocket | 6,749 | 8,184 | 0.8246× | 0.82× | ✅ |
| timer | 201,995 | 313,662 | 0.6440× | 0.65× | ✅ |
| app | 126,356 | 135,919 | 0.9296× | 0.93× | ✅ |
| http2 | 20,009 | 20,467 | 0.9776× | 0.98× | ✅ |
| table | 196,022 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 138,446 | 154,654 | 0.8952× | 0.91× | ✅ |
| task | 32,199 | 33,261 | 0.9681× | 0.97× | ✅ |
| mysql | 49,852 | 51,052 | 0.9765× | 0.98× | ✅ |
| http-client | 33,822 | 35,806 | 0.9446× | 0.95× | ✅ |
| client | 18,765 | 19,754 | 0.9500× | 0.95× | ✅ |
| socket | 19,732 | 21,067 | 0.9366× | 0.94× | ✅ |
| gzip | 196,176 | 197,083 | 0.9954× | 1.0× | ✅ |
| websocket-deflate | 181,508 | 181,568 | 0.9997× | 1.0× | ✅ |
| tcp-length | 5,239 | 4,762 | 1.1001× | 1.09× | ❌ |
| tcp-eof | 5,770 | 10,228 | 0.5641× | 0.56× | ✅ |
| udp | 7,997 | 7,986 | 1.0013× | 1.0× | ❌ |
| connect | 18,439 | 18,703 | 0.9858× | 0.99× | ✅ |
| https | 109,142 | 108,810 | 1.0031× | 1.01× | ➖ |
| wss | 104,704 | 104,800 | 0.9991× | 1.0× | ✅ |
| atomic | 49,969 | 51,316 | 0.9738× | 0.98× | ✅ |
| process-mode | 16,627 | 17,028 | 0.9764× | 0.98× | ✅ |
| http2-concurrent | 375,284 | 408,880 | 0.9178× | 0.92× | ✅ |
| websocket-64k | 156,195 | 159,693 | 0.9781× | 0.98× | ✅ |
| websocket-fragmented | 13,009 | 16,327 | 0.7968× | 0.79× | ✅ |
| upload | 544,409 | 1,322,382 | 0.4117× | 0.42× | ✅ |
| static | 34,368 | 41,010 | 0.8380× | 0.84× | ✅ |
| pgsql | 23,603 | 24,594 | 0.9597× | 0.96× | ✅ |
| sqlite | 23,839 | 25,232 | 0.9448× | 0.95× | ✅ |
| curl | 85,647 | 90,513 | 0.9462× | 0.95× | ✅ |
| dns | 231,169 | 232,637 | 0.9937× | 1.0× | ✅ |

Geometric mean 0.9046×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
