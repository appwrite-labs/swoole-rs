# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ce2d10f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,974 | 15,213 | 0.9186× | 0.93× | ✅ |
| echo-1k | 17,089 | 18,165 | 0.9407× | 0.95× | ✅ |
| json | 22,058 | 23,161 | 0.9524× | 0.96× | ✅ |
| service-health | 16,562 | 17,774 | 0.9318× | 0.94× | ✅ |
| websocket | 6,661 | 8,188 | 0.8135× | 0.82× | ✅ |
| timer | 202,023 | 313,792 | 0.6438× | 0.65× | ✅ |
| app | 125,594 | 135,949 | 0.9238× | 0.93× | ✅ |
| http2 | 19,689 | 20,468 | 0.9619× | 0.98× | ✅ |
| table | 195,771 | 201,865 | 0.9698× | 0.97× | ✅ |
| channel | 138,065 | 154,654 | 0.8927× | 0.9× | ✅ |
| task | 31,985 | 33,264 | 0.9615× | 0.97× | ✅ |
| mysql | 49,483 | 50,988 | 0.9705× | 0.98× | ✅ |
| http-client | 33,462 | 35,813 | 0.9343× | 0.95× | ✅ |
| client | 18,449 | 19,756 | 0.9338× | 0.95× | ✅ |
| socket | 19,419 | 21,067 | 0.9218× | 0.94× | ✅ |
| gzip | 195,945 | 197,083 | 0.9942× | 1.0× | ✅ |
| websocket-deflate | 181,419 | 181,572 | 0.9992× | 1.0× | ✅ |
| tcp-length | 4,955 | 4,762 | 1.0404× | 1.09× | ➖ |
| tcp-eof | 5,523 | 10,231 | 0.5398× | 0.56× | ✅ |
| udp | 7,995 | 7,986 | 1.0011× | 1.0× | ❌ |
| connect | 18,275 | 18,707 | 0.9769× | 0.99× | ✅ |
| https | 108,875 | 108,813 | 1.0006× | 1.01× | ➖ |
| wss | 104,596 | 104,794 | 0.9981× | 1.0× | ✅ |
| atomic | 49,722 | 51,316 | 0.9689× | 0.98× | ✅ |
| process-mode | 16,465 | 16,959 | 0.9709× | 0.98× | ✅ |
| http2-concurrent | 370,958 | 408,882 | 0.9072× | 0.92× | ✅ |
| websocket-64k | 156,111 | 159,697 | 0.9775× | 0.98× | ✅ |
| websocket-fragmented | 12,985 | 16,331 | 0.7951× | 0.79× | ✅ |
| upload | 544,191 | 1,322,380 | 0.4115× | 0.42× | ✅ |
| static | 34,422 | 41,039 | 0.8388× | 0.84× | ✅ |
| pgsql | 23,235 | 24,595 | 0.9447× | 0.96× | ✅ |
| sqlite | 23,525 | 25,232 | 0.9324× | 0.95× | ✅ |
| curl | 85,219 | 90,508 | 0.9416× | 0.95× | ✅ |
| dns | 231,003 | 233,068 | 0.9911× | 1.0× | ✅ |

Geometric mean 0.8959×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
