# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `da637b0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,923 | 15,214 | 0.9151× | 0.93× | ✅ |
| echo-1k | 17,040 | 18,165 | 0.9381× | 0.95× | ✅ |
| json | 22,015 | 23,161 | 0.9505× | 0.96× | ✅ |
| service-health | 16,506 | 17,773 | 0.9287× | 0.94× | ✅ |
| websocket | 6,669 | 8,191 | 0.8142× | 0.82× | ✅ |
| timer | 201,999 | 313,607 | 0.6441× | 0.65× | ✅ |
| app | 125,620 | 135,937 | 0.9241× | 0.93× | ✅ |
| http2 | 19,726 | 20,464 | 0.9639× | 0.98× | ✅ |
| table | 195,775 | 201,865 | 0.9698× | 0.97× | ✅ |
| channel | 138,009 | 154,654 | 0.8924× | 0.9× | ✅ |
| task | 31,930 | 33,268 | 0.9598× | 0.97× | ✅ |
| mysql | 49,440 | 51,062 | 0.9682× | 0.98× | ✅ |
| http-client | 33,368 | 35,809 | 0.9318× | 0.95× | ✅ |
| client | 18,393 | 19,757 | 0.9310× | 0.95× | ✅ |
| socket | 19,363 | 21,067 | 0.9191× | 0.94× | ✅ |
| gzip | 195,884 | 197,079 | 0.9939× | 1.0× | ✅ |
| websocket-deflate | 181,428 | 181,568 | 0.9992× | 1.0× | ✅ |
| tcp-length | 4,983 | 4,762 | 1.0463× | 1.09× | ➖ |
| tcp-eof | 5,525 | 10,231 | 0.5400× | 0.56× | ✅ |
| udp | 7,995 | 7,987 | 1.0010× | 1.0× | ❌ |
| connect | 18,216 | 18,707 | 0.9738× | 0.99× | ✅ |
| https | 108,833 | 108,810 | 1.0002× | 1.01× | ➖ |
| wss | 104,614 | 104,794 | 0.9983× | 1.0× | ✅ |
| atomic | 49,666 | 51,316 | 0.9678× | 0.98× | ✅ |
| process-mode | 16,420 | 17,021 | 0.9647× | 0.98× | ✅ |
| http2-concurrent | 372,097 | 408,880 | 0.9100× | 0.92× | ✅ |
| websocket-64k | 156,116 | 159,694 | 0.9776× | 0.98× | ✅ |
| websocket-fragmented | 13,024 | 16,327 | 0.7977× | 0.79× | ✅ |
| upload | 544,654 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,355 | 40,981 | 0.8383× | 0.84× | ✅ |
| pgsql | 23,176 | 24,595 | 0.9423× | 0.96× | ✅ |
| sqlite | 23,469 | 25,232 | 0.9301× | 0.95× | ✅ |
| curl | 85,154 | 90,513 | 0.9408× | 0.95× | ✅ |
| dns | 231,032 | 232,568 | 0.9934× | 1.0× | ✅ |

Geometric mean 0.8953×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
