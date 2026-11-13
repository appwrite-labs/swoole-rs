# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `fefac72` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,947 | 15,210 | 0.9170× | 0.92× | ✅ |
| echo-1k | 17,062 | 18,165 | 0.9393× | 0.94× | ✅ |
| json | 22,051 | 23,161 | 0.9521× | 0.95× | ✅ |
| service-health | 16,549 | 17,773 | 0.9311× | 0.93× | ✅ |
| websocket | 6,581 | 8,191 | 0.8034× | 0.81× | ✅ |
| timer | 203,458 | 313,543 | 0.6489× | 0.65× | ✅ |
| app | 125,297 | 135,915 | 0.9219× | 0.93× | ✅ |
| http2 | 18,688 | 20,464 | 0.9132× | 0.92× | ✅ |
| table | 195,498 | 201,865 | 0.9685× | 0.97× | ✅ |
| channel | 137,436 | 154,654 | 0.8887× | 0.9× | ✅ |
| task | 32,005 | 33,268 | 0.9620× | 0.96× | ✅ |
| mysql | 49,527 | 51,061 | 0.9700× | 0.97× | ✅ |
| http-client | 33,665 | 35,808 | 0.9402× | 0.94× | ✅ |
| client | 18,456 | 19,757 | 0.9341× | 0.94× | ✅ |
| socket | 19,584 | 21,067 | 0.9296× | 0.92× | ✅ |
| gzip | 194,148 | 197,083 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,391 | 181,572 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,792 | 4,765 | 1.0055× | 1.01× | ➖ |
| tcp-eof | 5,307 | 10,228 | 0.5188× | 0.52× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,395 | 18,707 | 0.9834× | 0.98× | ✅ |
| https | 108,802 | 108,803 | 1.0000× | 1.0× | ✅ |
| wss | 104,405 | 104,794 | 0.9963× | 1.0× | ✅ |
| atomic | 49,712 | 51,316 | 0.9687× | 0.97× | ✅ |
| process-mode | 15,855 | 16,959 | 0.9349× | 0.99× | ✅ |
| http2-concurrent | 359,891 | 408,877 | 0.8802× | 0.88× | ✅ |
| websocket-64k | 156,030 | 159,691 | 0.9771× | 0.98× | ✅ |
| websocket-fragmented | 12,815 | 16,331 | 0.7847× | 0.79× | ✅ |
| upload | 478,069 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,236 | 41,045 | 0.7123× | 0.72× | ✅ |
| pgsql | 23,209 | 24,594 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,472 | 25,232 | 0.9302× | 0.94× | ✅ |
| curl | 84,854 | 90,354 | 0.9391× | 0.94× | ✅ |
| dns | 230,873 | 233,190 | 0.9901× | 0.99× | ✅ |

Geometric mean 0.8807×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
