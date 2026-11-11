# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3a8703a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,979 | 15,214 | 0.9188× | 0.93× | ✅ |
| echo-1k | 17,092 | 18,162 | 0.9411× | 0.95× | ✅ |
| json | 22,061 | 23,158 | 0.9527× | 0.96× | ✅ |
| service-health | 16,562 | 17,777 | 0.9317× | 0.94× | ✅ |
| websocket | 6,661 | 8,188 | 0.8136× | 0.82× | ✅ |
| timer | 201,918 | 313,539 | 0.6440× | 0.65× | ✅ |
| app | 125,660 | 135,964 | 0.9242× | 0.93× | ✅ |
| http2 | 19,694 | 20,467 | 0.9622× | 0.98× | ✅ |
| table | 195,767 | 201,865 | 0.9698× | 0.97× | ✅ |
| channel | 138,065 | 154,654 | 0.8927× | 0.9× | ✅ |
| task | 31,982 | 33,268 | 0.9613× | 0.97× | ✅ |
| mysql | 49,483 | 51,064 | 0.9690× | 0.98× | ✅ |
| http-client | 33,462 | 35,805 | 0.9345× | 0.95× | ✅ |
| client | 18,452 | 19,757 | 0.9340× | 0.95× | ✅ |
| socket | 19,419 | 21,067 | 0.9218× | 0.94× | ✅ |
| gzip | 195,945 | 197,079 | 0.9942× | 1.0× | ✅ |
| websocket-deflate | 181,416 | 181,568 | 0.9992× | 1.0× | ✅ |
| tcp-length | 4,955 | 4,762 | 1.0404× | 1.09× | ➖ |
| tcp-eof | 5,523 | 10,228 | 0.5399× | 0.56× | ✅ |
| udp | 7,994 | 7,987 | 1.0009× | 1.0× | ❌ |
| connect | 18,272 | 18,707 | 0.9767× | 0.99× | ✅ |
| https | 108,861 | 108,803 | 1.0005× | 1.01× | ➖ |
| wss | 104,596 | 104,794 | 0.9981× | 1.0× | ✅ |
| atomic | 49,722 | 51,316 | 0.9689× | 0.98× | ✅ |
| process-mode | 16,459 | 16,959 | 0.9705× | 0.98× | ✅ |
| http2-concurrent | 370,984 | 409,564 | 0.9058× | 0.92× | ✅ |
| websocket-64k | 156,108 | 159,697 | 0.9775× | 0.98× | ✅ |
| websocket-fragmented | 12,988 | 16,327 | 0.7955× | 0.79× | ✅ |
| upload | 544,186 | 1,322,380 | 0.4115× | 0.42× | ✅ |
| static | 34,355 | 40,981 | 0.8383× | 0.84× | ✅ |
| pgsql | 23,233 | 24,595 | 0.9446× | 0.96× | ✅ |
| sqlite | 23,525 | 25,232 | 0.9324× | 0.95× | ✅ |
| curl | 85,218 | 90,512 | 0.9415× | 0.95× | ✅ |
| dns | 231,412 | 232,542 | 0.9951× | 1.0× | ✅ |

Geometric mean 0.8959×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
