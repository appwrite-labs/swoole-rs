# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,214 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,754 | 18,165 | 0.9223× | 0.92× | ✅ |
| json | 22,050 | 23,173 | 0.9515× | 0.95× | ✅ |
| service-health | 16,305 | 17,786 | 0.9167× | 0.92× | ✅ |
| websocket | 6,740 | 8,189 | 0.8231× | 0.81× | ✅ |
| timer | 202,096 | 313,932 | 0.6438× | 0.65× | ✅ |
| app | 127,534 | 135,975 | 0.9379× | 0.93× | ✅ |
| http2 | 19,176 | 20,496 | 0.9356× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,723 | 0.8799× | 0.88× | ✅ |
| task | 32,076 | 33,261 | 0.9644× | 0.96× | ✅ |
| mysql | 50,290 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,656 | 35,792 | 0.9403× | 0.93× | ✅ |
| gzip | 194,290 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,572 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,420 | 51,285 | 0.9831× | 0.97× | ✅ |
| process-mode | 15,407 | 17,027 | 0.9049× | 0.89× | ✅ |
| http2-concurrent | 366,870 | 409,057 | 0.8969× | 0.89× | ✅ |
| websocket-64k | 90,038 | 159,695 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,368 | 0.7310× | 0.72× | ✅ |
| upload | 418,896 | 1,322,428 | 0.3168× | 0.32× | ✅ |
| static | 25,364 | 40,999 | 0.6186× | 0.62× | ✅ |
| client | 18,596 | 19,729 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,889 | 24,663 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,074 | 90,422 | 0.9519× | 0.95× | ✅ |
| dns | 230,324 | 232,765 | 0.9895× | 0.99× | ✅ |
| file | 108,876 | 131,300 | 0.8292× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
