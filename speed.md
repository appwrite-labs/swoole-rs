# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2e5bba2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,915 | 15,213 | 0.9147× | 0.92× | ✅ |
| echo-1k | 17,018 | 18,165 | 0.9368× | 0.94× | ✅ |
| json | 22,015 | 23,161 | 0.9505× | 0.95× | ✅ |
| service-health | 16,509 | 17,777 | 0.9287× | 0.93× | ✅ |
| websocket | 6,564 | 8,184 | 0.8021× | 0.82× | ✅ |
| timer | 203,598 | 313,899 | 0.6486× | 0.65× | ✅ |
| app | 125,760 | 135,922 | 0.9252× | 0.93× | ✅ |
| http2 | 18,729 | 20,463 | 0.9152× | 0.93× | ✅ |
| table | 195,716 | 201,865 | 0.9695× | 0.97× | ✅ |
| channel | 138,861 | 154,654 | 0.8979× | 0.9× | ✅ |
| task | 32,008 | 33,265 | 0.9622× | 0.96× | ✅ |
| mysql | 49,532 | 51,061 | 0.9701× | 0.97× | ✅ |
| http-client | 33,609 | 35,809 | 0.9386× | 0.94× | ✅ |
| client | 18,441 | 19,753 | 0.9336× | 0.94× | ✅ |
| socket | 19,561 | 21,067 | 0.9285× | 0.92× | ✅ |
| gzip | 196,007 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,329 | 181,568 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,761 | 4,762 | 0.9997× | 1.05× | ✅ |
| tcp-eof | 5,308 | 10,228 | 0.5190× | 0.54× | ✅ |
| udp | 7,543 | 7,987 | 0.9444× | 0.95× | ✅ |
| connect | 18,203 | 18,707 | 0.9731× | 0.98× | ✅ |
| https | 108,767 | 108,803 | 0.9997× | 1.0× | ✅ |
| wss | 104,459 | 104,794 | 0.9968× | 1.0× | ✅ |
| atomic | 49,667 | 51,312 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,700 | 16,957 | 0.9849× | 0.97× | ✅ |
| http2-concurrent | 359,349 | 408,882 | 0.8789× | 0.88× | ✅ |
| websocket-64k | 156,014 | 159,696 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,804 | 16,327 | 0.7842× | 0.79× | ✅ |
| upload | 478,410 | 1,322,382 | 0.3618× | 0.37× | ✅ |
| static | 29,180 | 41,010 | 0.7115× | 0.72× | ✅ |
| pgsql | 23,180 | 24,594 | 0.9425× | 0.95× | ✅ |
| sqlite | 23,456 | 25,221 | 0.9300× | 0.94× | ✅ |
| curl | 84,734 | 90,479 | 0.9365× | 0.94× | ✅ |
| dns | 230,882 | 232,625 | 0.9925× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
