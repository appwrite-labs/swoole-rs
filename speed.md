# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a55bcbd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,008 | 15,214 | 0.9207× | 0.93× | ✅ |
| echo-1k | 17,133 | 18,165 | 0.9432× | 0.95× | ✅ |
| json | 22,118 | 23,161 | 0.9550× | 0.96× | ✅ |
| service-health | 16,608 | 17,777 | 0.9343× | 0.94× | ✅ |
| websocket | 6,673 | 8,185 | 0.8152× | 0.82× | ✅ |
| timer | 202,081 | 313,552 | 0.6445× | 0.65× | ✅ |
| app | 125,716 | 135,950 | 0.9247× | 0.93× | ✅ |
| http2 | 19,752 | 20,468 | 0.9650× | 0.98× | ✅ |
| table | 195,821 | 201,865 | 0.9701× | 0.97× | ✅ |
| channel | 138,245 | 154,654 | 0.8939× | 0.9× | ✅ |
| task | 32,015 | 33,268 | 0.9623× | 0.97× | ✅ |
| mysql | 49,532 | 51,064 | 0.9700× | 0.98× | ✅ |
| http-client | 33,483 | 35,806 | 0.9351× | 0.95× | ✅ |
| client | 18,505 | 19,757 | 0.9366× | 0.95× | ✅ |
| socket | 19,468 | 21,067 | 0.9241× | 0.94× | ✅ |
| gzip | 195,989 | 197,079 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,425 | 181,568 | 0.9992× | 1.0× | ✅ |
| tcp-length | 5,170 | 4,762 | 1.0857× | 1.09× | ➖ |
| tcp-eof | 5,705 | 10,231 | 0.5576× | 0.56× | ✅ |
| udp | 7,918 | 7,986 | 0.9914× | 1.0× | ✅ |
| connect | 18,140 | 18,707 | 0.9697× | 0.99× | ✅ |
| https | 108,885 | 108,803 | 1.0008× | 1.01× | ➖ |
| wss | 104,580 | 104,800 | 0.9979× | 1.0× | ✅ |
| atomic | 49,772 | 51,312 | 0.9700× | 0.98× | ✅ |
| process-mode | 16,419 | 16,959 | 0.9682× | 0.98× | ✅ |
| http2-concurrent | 372,775 | 408,880 | 0.9117× | 0.92× | ✅ |
| websocket-64k | 156,115 | 159,692 | 0.9776× | 0.98× | ✅ |
| websocket-fragmented | 12,715 | 16,331 | 0.7786× | 0.79× | ✅ |
| upload | 544,226 | 1,322,385 | 0.4115× | 0.42× | ✅ |
| static | 34,112 | 41,010 | 0.8318× | 0.84× | ✅ |
| pgsql | 23,266 | 24,594 | 0.9460× | 0.96× | ✅ |
| sqlite | 23,553 | 25,232 | 0.9335× | 0.95× | ✅ |
| curl | 85,246 | 90,383 | 0.9432× | 0.95× | ✅ |
| dns | 231,130 | 232,550 | 0.9939× | 1.0× | ✅ |

Geometric mean 0.8976×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
