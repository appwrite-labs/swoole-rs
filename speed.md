# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5f9801b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,919 | 15,214 | 0.9149× | 0.93× | ✅ |
| echo-1k | 17,040 | 18,162 | 0.9382× | 0.95× | ✅ |
| json | 22,001 | 23,158 | 0.9501× | 0.96× | ✅ |
| service-health | 16,506 | 17,773 | 0.9287× | 0.94× | ✅ |
| websocket | 6,673 | 8,184 | 0.8153× | 0.82× | ✅ |
| timer | 202,051 | 313,886 | 0.6437× | 0.65× | ✅ |
| app | 125,555 | 135,869 | 0.9241× | 0.93× | ✅ |
| http2 | 19,728 | 20,464 | 0.9640× | 0.98× | ✅ |
| table | 195,779 | 201,865 | 0.9698× | 0.97× | ✅ |
| channel | 138,009 | 154,651 | 0.8924× | 0.9× | ✅ |
| task | 31,930 | 33,265 | 0.9599× | 0.97× | ✅ |
| mysql | 49,423 | 51,064 | 0.9679× | 0.98× | ✅ |
| http-client | 33,432 | 35,806 | 0.9337× | 0.95× | ✅ |
| client | 18,396 | 19,757 | 0.9311× | 0.95× | ✅ |
| socket | 19,363 | 21,067 | 0.9191× | 0.94× | ✅ |
| gzip | 195,884 | 197,079 | 0.9939× | 1.0× | ✅ |
| websocket-deflate | 181,428 | 181,568 | 0.9992× | 1.0× | ✅ |
| tcp-length | 4,983 | 4,765 | 1.0456× | 1.09× | ➖ |
| tcp-eof | 5,525 | 10,231 | 0.5400× | 0.56× | ✅ |
| udp | 7,995 | 7,987 | 1.0010× | 1.0× | ❌ |
| connect | 18,219 | 18,707 | 0.9739× | 0.99× | ✅ |
| https | 108,823 | 108,810 | 1.0001× | 1.01× | ➖ |
| wss | 104,621 | 104,794 | 0.9984× | 1.0× | ✅ |
| atomic | 49,662 | 51,316 | 0.9678× | 0.98× | ✅ |
| process-mode | 16,406 | 16,956 | 0.9676× | 0.98× | ✅ |
| http2-concurrent | 371,631 | 408,877 | 0.9089× | 0.92× | ✅ |
| websocket-64k | 156,122 | 159,694 | 0.9776× | 0.98× | ✅ |
| websocket-fragmented | 13,024 | 16,331 | 0.7975× | 0.79× | ✅ |
| upload | 544,652 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,477 | 41,103 | 0.8388× | 0.84× | ✅ |
| pgsql | 23,173 | 24,595 | 0.9422× | 0.96× | ✅ |
| sqlite | 23,469 | 25,232 | 0.9301× | 0.95× | ✅ |
| curl | 85,162 | 90,376 | 0.9423× | 0.95× | ✅ |
| dns | 231,063 | 233,162 | 0.9910× | 1.0× | ✅ |

Geometric mean 0.8953×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
