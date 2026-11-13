# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `501dea7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,960 | 15,214 | 0.9176× | 0.92× | ✅ |
| echo-1k | 17,063 | 18,165 | 0.9393× | 0.94× | ✅ |
| json | 22,061 | 23,158 | 0.9526× | 0.95× | ✅ |
| service-health | 16,563 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,584 | 8,188 | 0.8042× | 0.82× | ✅ |
| timer | 203,604 | 313,599 | 0.6493× | 0.65× | ✅ |
| app | 125,537 | 135,808 | 0.9244× | 0.93× | ✅ |
| http2 | 18,654 | 20,465 | 0.9115× | 0.93× | ✅ |
| table | 195,475 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 138,439 | 154,654 | 0.8951× | 0.9× | ✅ |
| task | 32,142 | 33,261 | 0.9663× | 0.96× | ✅ |
| mysql | 49,555 | 51,064 | 0.9704× | 0.97× | ✅ |
| http-client | 33,744 | 35,809 | 0.9423× | 0.94× | ✅ |
| client | 18,477 | 19,757 | 0.9352× | 0.94× | ✅ |
| socket | 19,615 | 21,067 | 0.9311× | 0.92× | ✅ |
| gzip | 194,161 | 197,079 | 0.9852× | 1.0× | ✅ |
| websocket-deflate | 179,401 | 181,572 | 0.9880× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,762 | 1.0063× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,231 | 0.5219× | 0.54× | ✅ |
| udp | 7,573 | 7,986 | 0.9482× | 0.95× | ✅ |
| connect | 18,374 | 18,707 | 0.9822× | 0.98× | ✅ |
| https | 108,795 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,495 | 104,803 | 0.9971× | 1.0× | ✅ |
| atomic | 49,682 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,680 | 16,962 | 0.9833× | 0.99× | ✅ |
| http2-concurrent | 359,010 | 408,877 | 0.8780× | 0.88× | ✅ |
| websocket-64k | 156,024 | 159,693 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 13,032 | 16,331 | 0.7980× | 0.79× | ✅ |
| upload | 478,024 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,362 | 41,132 | 0.7138× | 0.72× | ✅ |
| pgsql | 23,181 | 24,595 | 0.9425× | 0.95× | ✅ |
| sqlite | 23,470 | 25,236 | 0.9300× | 0.94× | ✅ |
| curl | 84,738 | 90,307 | 0.9383× | 0.94× | ✅ |
| dns | 230,916 | 232,844 | 0.9917× | 0.99× | ✅ |

Geometric mean 0.8831×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
