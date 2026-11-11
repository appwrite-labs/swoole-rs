# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `26069a0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,230 | 15,210 | 0.9356× | 0.93× | ✅ |
| echo-1k | 17,339 | 18,162 | 0.9547× | 0.95× | ✅ |
| json | 22,326 | 23,162 | 0.9639× | 0.96× | ✅ |
| service-health | 16,824 | 17,774 | 0.9465× | 0.94× | ✅ |
| websocket | 6,679 | 8,188 | 0.8158× | 0.83× | ✅ |
| timer | 202,189 | 313,729 | 0.6445× | 0.65× | ✅ |
| app | 126,523 | 135,883 | 0.9311× | 0.93× | ✅ |
| http2 | 19,997 | 20,468 | 0.9770× | 0.98× | ✅ |
| table | 196,318 | 201,865 | 0.9725× | 0.97× | ✅ |
| channel | 140,012 | 154,654 | 0.9053× | 0.91× | ✅ |
| task | 32,113 | 33,265 | 0.9654× | 0.97× | ✅ |
| mysql | 49,842 | 51,014 | 0.9770× | 0.98× | ✅ |
| http-client | 34,051 | 35,806 | 0.9510× | 0.95× | ✅ |
| client | 18,816 | 19,757 | 0.9524× | 0.95× | ✅ |
| socket | 19,795 | 21,067 | 0.9396× | 0.94× | ✅ |
| gzip | 196,193 | 197,080 | 0.9955× | 1.0× | ✅ |
| websocket-deflate | 181,444 | 181,572 | 0.9993× | 1.0× | ✅ |
| tcp-length | 5,182 | 4,762 | 1.0881× | 1.1× | ➖ |
| tcp-eof | 5,713 | 10,231 | 0.5583× | 0.57× | ✅ |
| udp | 7,961 | 7,986 | 0.9968× | 1.0× | ✅ |
| connect | 18,392 | 18,707 | 0.9831× | 0.99× | ✅ |
| https | 109,144 | 108,810 | 1.0031× | 1.01× | ➖ |
| wss | 104,585 | 104,800 | 0.9979× | 1.0× | ✅ |
| atomic | 50,017 | 51,312 | 0.9748× | 0.98× | ✅ |
| process-mode | 16,636 | 16,956 | 0.9811× | 0.98× | ✅ |
| http2-concurrent | 375,007 | 408,877 | 0.9172× | 0.92× | ✅ |
| websocket-64k | 156,127 | 159,691 | 0.9777× | 0.98× | ✅ |
| websocket-fragmented | 12,931 | 16,331 | 0.7918× | 0.79× | ✅ |
| upload | 544,449 | 1,322,381 | 0.4117× | 0.42× | ✅ |
| static | 34,203 | 41,010 | 0.8340× | 0.85× | ✅ |
| pgsql | 23,501 | 24,596 | 0.9554× | 0.96× | ✅ |
| sqlite | 23,848 | 25,232 | 0.9452× | 0.95× | ✅ |
| curl | 85,769 | 90,330 | 0.9495× | 0.95× | ✅ |
| dns | 231,747 | 232,569 | 0.9965× | 1.0× | ✅ |

Geometric mean 0.9042×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
