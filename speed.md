# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4da5fc2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,916 | 15,213 | 0.9147× | 0.92× | ✅ |
| echo-1k | 17,021 | 18,165 | 0.9370× | 0.94× | ✅ |
| json | 22,009 | 23,161 | 0.9503× | 0.95× | ✅ |
| service-health | 16,511 | 17,777 | 0.9288× | 0.93× | ✅ |
| websocket | 6,573 | 8,191 | 0.8025× | 0.82× | ✅ |
| timer | 203,348 | 313,717 | 0.6482× | 0.65× | ✅ |
| app | 125,723 | 136,009 | 0.9244× | 0.93× | ✅ |
| http2 | 18,667 | 20,463 | 0.9122× | 0.93× | ✅ |
| table | 195,468 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 137,395 | 154,651 | 0.8884× | 0.9× | ✅ |
| task | 32,149 | 33,268 | 0.9664× | 0.96× | ✅ |
| mysql | 49,575 | 50,962 | 0.9728× | 0.97× | ✅ |
| http-client | 33,631 | 35,805 | 0.9393× | 0.94× | ✅ |
| client | 18,472 | 19,757 | 0.9350× | 0.94× | ✅ |
| socket | 19,586 | 21,064 | 0.9299× | 0.92× | ✅ |
| gzip | 194,113 | 197,083 | 0.9849× | 1.0× | ✅ |
| websocket-deflate | 179,379 | 181,572 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,788 | 4,765 | 1.0047× | 1.05× | ➖ |
| tcp-eof | 5,336 | 10,231 | 0.5215× | 0.54× | ✅ |
| udp | 7,576 | 7,987 | 0.9486× | 0.95× | ✅ |
| connect | 18,353 | 18,707 | 0.9811× | 0.98× | ✅ |
| https | 108,806 | 108,813 | 0.9999× | 1.0× | ✅ |
| wss | 104,468 | 104,794 | 0.9969× | 1.0× | ✅ |
| atomic | 49,675 | 51,312 | 0.9681× | 0.97× | ✅ |
| process-mode | 16,671 | 16,959 | 0.9830× | 0.99× | ✅ |
| http2-concurrent | 359,081 | 408,882 | 0.8782× | 0.88× | ✅ |
| websocket-64k | 156,020 | 159,699 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,812 | 16,331 | 0.7845× | 0.79× | ✅ |
| upload | 478,504 | 1,322,380 | 0.3619× | 0.37× | ✅ |
| static | 29,229 | 41,010 | 0.7127× | 0.72× | ✅ |
| pgsql | 23,203 | 24,595 | 0.9434× | 0.95× | ✅ |
| sqlite | 23,487 | 25,236 | 0.9307× | 0.94× | ✅ |
| curl | 84,755 | 90,355 | 0.9380× | 0.94× | ✅ |
| dns | 230,938 | 233,052 | 0.9909× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
