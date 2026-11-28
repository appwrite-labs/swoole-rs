# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,214 | 0.9017× | 0.91× | ✅ |
| echo-1k | 16,754 | 18,166 | 0.9223× | 0.92× | ✅ |
| json | 22,050 | 23,173 | 0.9515× | 0.95× | ✅ |
| service-health | 16,308 | 17,786 | 0.9169× | 0.92× | ✅ |
| websocket | 6,736 | 8,192 | 0.8222× | 0.81× | ✅ |
| timer | 202,488 | 314,631 | 0.6436× | 0.65× | ✅ |
| app | 127,555 | 135,962 | 0.9382× | 0.93× | ✅ |
| http2 | 19,172 | 20,496 | 0.9354× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,072 | 33,269 | 0.9640× | 0.96× | ✅ |
| mysql | 50,294 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,653 | 35,794 | 0.9402× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,561 | 104,800 | 0.9691× | 0.97× | ✅ |
| atomic | 50,423 | 51,276 | 0.9834× | 0.97× | ✅ |
| process-mode | 15,403 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 366,773 | 408,864 | 0.8971× | 0.89× | ✅ |
| websocket-64k | 90,037 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,893 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,439 | 41,063 | 0.6195× | 0.62× | ✅ |
| client | 18,595 | 19,721 | 0.9429× | 0.93× | ✅ |
| socket | 19,835 | 21,036 | 0.9429× | 0.93× | ✅ |
| pgsql | 22,890 | 24,663 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,068 | 90,546 | 0.9505× | 0.95× | ✅ |
| dns | 230,181 | 232,742 | 0.9890× | 0.99× | ✅ |
| file | 108,880 | 130,917 | 0.8317× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
