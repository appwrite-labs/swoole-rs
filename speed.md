# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `155d523` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,915 | 15,213 | 0.9147× | 0.92× | ✅ |
| echo-1k | 17,018 | 18,165 | 0.9368× | 0.94× | ✅ |
| json | 22,018 | 23,161 | 0.9507× | 0.95× | ✅ |
| service-health | 16,509 | 17,777 | 0.9287× | 0.93× | ✅ |
| websocket | 6,569 | 8,188 | 0.8023× | 0.82× | ✅ |
| timer | 203,386 | 313,526 | 0.6487× | 0.65× | ✅ |
| app | 125,841 | 135,840 | 0.9264× | 0.93× | ✅ |
| http2 | 18,731 | 20,468 | 0.9152× | 0.93× | ✅ |
| table | 195,716 | 201,865 | 0.9695× | 0.97× | ✅ |
| channel | 138,861 | 154,654 | 0.8979× | 0.9× | ✅ |
| task | 32,003 | 33,268 | 0.9620× | 0.96× | ✅ |
| mysql | 49,580 | 51,049 | 0.9712× | 0.97× | ✅ |
| http-client | 33,671 | 35,806 | 0.9404× | 0.94× | ✅ |
| client | 18,466 | 19,758 | 0.9346× | 0.94× | ✅ |
| socket | 19,584 | 21,064 | 0.9298× | 0.92× | ✅ |
| gzip | 195,992 | 197,083 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,332 | 181,572 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,761 | 4,762 | 0.9997× | 1.05× | ✅ |
| tcp-eof | 5,308 | 10,228 | 0.5190× | 0.54× | ✅ |
| udp | 7,543 | 7,986 | 0.9445× | 0.95× | ✅ |
| connect | 18,206 | 18,707 | 0.9733× | 0.98× | ✅ |
| https | 108,767 | 108,810 | 0.9996× | 1.0× | ✅ |
| wss | 104,445 | 104,794 | 0.9967× | 1.0× | ✅ |
| atomic | 49,667 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,702 | 17,024 | 0.9811× | 0.97× | ✅ |
| http2-concurrent | 359,235 | 408,877 | 0.8786× | 0.88× | ✅ |
| websocket-64k | 156,008 | 159,692 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,800 | 16,331 | 0.7838× | 0.79× | ✅ |
| upload | 478,404 | 1,322,382 | 0.3618× | 0.37× | ✅ |
| static | 29,209 | 41,010 | 0.7122× | 0.72× | ✅ |
| pgsql | 23,185 | 24,594 | 0.9427× | 0.95× | ✅ |
| sqlite | 23,456 | 25,236 | 0.9295× | 0.94× | ✅ |
| curl | 84,734 | 90,351 | 0.9378× | 0.94× | ✅ |
| dns | 230,705 | 233,355 | 0.9886× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
