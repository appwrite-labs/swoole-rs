# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ec813f7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,933 | 15,210 | 0.9161× | 0.92× | ✅ |
| echo-1k | 17,038 | 18,165 | 0.9379× | 0.94× | ✅ |
| json | 22,031 | 23,161 | 0.9512× | 0.95× | ✅ |
| service-health | 16,526 | 17,773 | 0.9298× | 0.93× | ✅ |
| websocket | 6,573 | 8,191 | 0.8024× | 0.82× | ✅ |
| timer | 203,287 | 313,700 | 0.6480× | 0.65× | ✅ |
| app | 125,795 | 135,907 | 0.9256× | 0.93× | ✅ |
| http2 | 18,638 | 20,465 | 0.9107× | 0.93× | ✅ |
| table | 195,736 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,884 | 154,651 | 0.8980× | 0.9× | ✅ |
| task | 32,174 | 33,264 | 0.9672× | 0.96× | ✅ |
| mysql | 49,600 | 51,061 | 0.9714× | 0.97× | ✅ |
| http-client | 33,681 | 35,809 | 0.9406× | 0.94× | ✅ |
| client | 18,486 | 19,757 | 0.9357× | 0.94× | ✅ |
| socket | 19,601 | 21,067 | 0.9304× | 0.92× | ✅ |
| gzip | 196,017 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,340 | 181,572 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,776 | 4,765 | 1.0022× | 1.05× | ➖ |
| tcp-eof | 5,324 | 10,231 | 0.5203× | 0.54× | ✅ |
| udp | 7,560 | 7,986 | 0.9466× | 0.95× | ✅ |
| connect | 18,367 | 18,707 | 0.9819× | 0.98× | ✅ |
| https | 108,777 | 108,813 | 0.9997× | 1.0× | ✅ |
| wss | 104,464 | 104,804 | 0.9968× | 1.0× | ✅ |
| atomic | 49,683 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,784 | 17,025 | 0.9859× | 0.99× | ✅ |
| http2-concurrent | 359,487 | 408,880 | 0.8792× | 0.88× | ✅ |
| websocket-64k | 156,022 | 159,695 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,808 | 16,331 | 0.7843× | 0.79× | ✅ |
| upload | 478,453 | 1,322,381 | 0.3618× | 0.37× | ✅ |
| static | 29,227 | 41,039 | 0.7122× | 0.72× | ✅ |
| pgsql | 23,193 | 24,595 | 0.9430× | 0.95× | ✅ |
| sqlite | 23,482 | 25,221 | 0.9311× | 0.94× | ✅ |
| curl | 84,712 | 90,351 | 0.9376× | 0.94× | ✅ |
| dns | 230,305 | 233,007 | 0.9884× | 0.99× | ✅ |

Geometric mean 0.8828×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
