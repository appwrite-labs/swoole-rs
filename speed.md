# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ad39ab6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,752 | 15,210 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,760 | 18,162 | 0.9228× | 0.92× | ✅ |
| json | 22,076 | 23,170 | 0.9528× | 0.95× | ✅ |
| service-health | 16,334 | 17,782 | 0.9186× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8152× | 0.81× | ✅ |
| timer | 201,477 | 313,741 | 0.6422× | 0.65× | ✅ |
| app | 126,977 | 135,896 | 0.9344× | 0.93× | ✅ |
| http2 | 18,965 | 20,497 | 0.9253× | 0.92× | ✅ |
| table | 194,301 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,357 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 31,964 | 33,268 | 0.9608× | 0.96× | ✅ |
| mysql | 50,048 | 51,085 | 0.9797× | 0.97× | ✅ |
| http-client | 33,374 | 35,796 | 0.9323× | 0.93× | ✅ |
| gzip | 194,366 | 197,095 | 0.9862× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,766 | 0.9748× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,986 | 0.9467× | 0.95× | ✅ |
| connect | 17,585 | 18,734 | 0.9387× | 0.93× | ✅ |
| https | 106,004 | 108,809 | 0.9742× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,187 | 51,281 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,272 | 17,021 | 0.8972× | 0.89× | ✅ |
| http2-concurrent | 364,853 | 409,057 | 0.8919× | 0.89× | ✅ |
| websocket-64k | 89,974 | 159,692 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,835 | 16,372 | 0.7229× | 0.72× | ✅ |
| upload | 416,434 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,115 | 41,063 | 0.6603× | 0.65× | ✅ |
| client | 18,476 | 19,729 | 0.9365× | 0.93× | ✅ |
| socket | 19,704 | 21,045 | 0.9363× | 0.93× | ✅ |
| pgsql | 23,094 | 24,667 | 0.9362× | 0.92× | ✅ |
| sqlite | 23,440 | 25,253 | 0.9282× | 0.93× | ✅ |
| curl | 85,928 | 90,417 | 0.9504× | 0.95× | ✅ |
| dns | 229,955 | 232,357 | 0.9897× | 0.99× | ✅ |
| file | 108,196 | 131,296 | 0.8241× | 0.83× | ✅ |

Geometric mean 0.8533×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
