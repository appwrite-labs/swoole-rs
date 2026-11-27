# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4d4694d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,214 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,754 | 18,165 | 0.9223× | 0.92× | ✅ |
| json | 22,051 | 23,173 | 0.9516× | 0.95× | ✅ |
| service-health | 16,305 | 17,786 | 0.9167× | 0.92× | ✅ |
| websocket | 6,736 | 8,189 | 0.8226× | 0.81× | ✅ |
| timer | 202,086 | 313,713 | 0.6442× | 0.65× | ✅ |
| app | 127,584 | 135,997 | 0.9381× | 0.93× | ✅ |
| http2 | 19,182 | 20,500 | 0.9357× | 0.92× | ✅ |
| table | 194,277 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,072 | 33,269 | 0.9640× | 0.96× | ✅ |
| mysql | 50,287 | 51,088 | 0.9843× | 0.97× | ✅ |
| http-client | 33,661 | 35,784 | 0.9407× | 0.93× | ✅ |
| gzip | 194,278 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,562 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,572 | 104,800 | 0.9692× | 0.97× | ✅ |
| atomic | 50,420 | 51,276 | 0.9833× | 0.97× | ✅ |
| process-mode | 15,403 | 17,025 | 0.9047× | 0.89× | ✅ |
| http2-concurrent | 367,882 | 408,865 | 0.8998× | 0.89× | ✅ |
| websocket-64k | 90,033 | 159,694 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,368 | 0.7311× | 0.72× | ✅ |
| upload | 418,897 | 1,322,425 | 0.3168× | 0.32× | ✅ |
| static | 25,390 | 41,028 | 0.6188× | 0.62× | ✅ |
| client | 18,598 | 19,718 | 0.9432× | 0.93× | ✅ |
| socket | 19,835 | 21,036 | 0.9429× | 0.93× | ✅ |
| pgsql | 22,885 | 24,666 | 0.9278× | 0.92× | ✅ |
| sqlite | 23,478 | 25,253 | 0.9297× | 0.93× | ✅ |
| curl | 86,076 | 90,546 | 0.9506× | 0.95× | ✅ |
| dns | 230,247 | 233,043 | 0.9880× | 0.99× | ✅ |
| file | 108,880 | 131,297 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
