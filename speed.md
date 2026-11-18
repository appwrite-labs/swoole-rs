# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `910a885` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,737 | 15,210 | 0.9032× | 0.91× | ✅ |
| echo-1k | 16,738 | 18,162 | 0.9216× | 0.92× | ✅ |
| json | 22,067 | 23,173 | 0.9522× | 0.95× | ✅ |
| service-health | 16,313 | 17,782 | 0.9174× | 0.92× | ✅ |
| websocket | 6,671 | 8,192 | 0.8143× | 0.81× | ✅ |
| timer | 201,484 | 314,029 | 0.6416× | 0.65× | ✅ |
| app | 127,056 | 135,912 | 0.9348× | 0.93× | ✅ |
| http2 | 18,962 | 20,496 | 0.9252× | 0.92× | ✅ |
| table | 193,898 | 201,885 | 0.9604× | 0.96× | ✅ |
| channel | 137,968 | 155,726 | 0.8860× | 0.88× | ✅ |
| task | 31,950 | 33,268 | 0.9604× | 0.96× | ✅ |
| mysql | 50,064 | 51,085 | 0.9800× | 0.97× | ✅ |
| http-client | 33,395 | 35,787 | 0.9331× | 0.93× | ✅ |
| gzip | 194,348 | 197,099 | 0.9860× | 0.99× | ✅ |
| websocket-deflate | 179,102 | 181,565 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,642 | 4,763 | 0.9746× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,231 | 0.5077× | 0.5× | ✅ |
| udp | 7,570 | 7,986 | 0.9479× | 0.95× | ✅ |
| connect | 17,580 | 18,734 | 0.9384× | 0.93× | ✅ |
| https | 106,028 | 108,809 | 0.9744× | 0.98× | ✅ |
| wss | 101,237 | 104,797 | 0.9660× | 0.97× | ✅ |
| atomic | 50,165 | 51,285 | 0.9782× | 0.97× | ✅ |
| process-mode | 15,264 | 17,028 | 0.8964× | 0.89× | ✅ |
| http2-concurrent | 364,900 | 409,057 | 0.8921× | 0.89× | ✅ |
| websocket-64k | 89,971 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,838 | 16,368 | 0.7232× | 0.72× | ✅ |
| upload | 416,423 | 1,322,425 | 0.3149× | 0.32× | ✅ |
| static | 27,041 | 40,999 | 0.6595× | 0.65× | ✅ |
| client | 18,469 | 19,729 | 0.9361× | 0.93× | ✅ |
| socket | 19,695 | 21,041 | 0.9360× | 0.93× | ✅ |
| pgsql | 22,929 | 24,619 | 0.9314× | 0.92× | ✅ |
| sqlite | 23,426 | 25,236 | 0.9283× | 0.93× | ✅ |
| curl | 85,411 | 90,354 | 0.9453× | 0.95× | ✅ |
| dns | 229,502 | 233,210 | 0.9841× | 0.99× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
