# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `10814c1` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,743 | 15,214 | 0.9033× | 0.91× | ✅ |
| echo-1k | 16,739 | 18,165 | 0.9215× | 0.92× | ✅ |
| json | 22,072 | 23,173 | 0.9525× | 0.95× | ✅ |
| service-health | 16,318 | 17,786 | 0.9175× | 0.92× | ✅ |
| websocket | 6,681 | 8,189 | 0.8158× | 0.81× | ✅ |
| timer | 201,684 | 313,792 | 0.6427× | 0.65× | ✅ |
| app | 127,063 | 135,892 | 0.9350× | 0.93× | ✅ |
| http2 | 18,974 | 20,497 | 0.9257× | 0.92× | ✅ |
| table | 193,903 | 201,885 | 0.9605× | 0.96× | ✅ |
| channel | 137,969 | 155,723 | 0.8860× | 0.88× | ✅ |
| task | 31,950 | 33,269 | 0.9604× | 0.96× | ✅ |
| mysql | 50,065 | 51,082 | 0.9801× | 0.97× | ✅ |
| http-client | 33,401 | 35,790 | 0.9333× | 0.93× | ✅ |
| gzip | 194,361 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,763 | 0.9750× | 0.96× | ✅ |
| tcp-eof | 5,201 | 10,228 | 0.5085× | 0.5× | ✅ |
| udp | 7,580 | 7,987 | 0.9490× | 0.95× | ✅ |
| connect | 17,581 | 18,734 | 0.9385× | 0.93× | ✅ |
| https | 106,023 | 108,809 | 0.9744× | 0.98× | ✅ |
| wss | 101,243 | 104,797 | 0.9661× | 0.97× | ✅ |
| atomic | 50,170 | 51,276 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,264 | 17,021 | 0.8967× | 0.89× | ✅ |
| http2-concurrent | 364,151 | 408,865 | 0.8906× | 0.89× | ✅ |
| websocket-64k | 89,978 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,844 | 16,372 | 0.7234× | 0.72× | ✅ |
| upload | 416,434 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,041 | 40,999 | 0.6595× | 0.65× | ✅ |
| client | 18,471 | 19,721 | 0.9366× | 0.93× | ✅ |
| socket | 19,701 | 21,037 | 0.9365× | 0.93× | ✅ |
| pgsql | 22,918 | 24,617 | 0.9310× | 0.92× | ✅ |
| sqlite | 23,403 | 25,236 | 0.9274× | 0.93× | ✅ |
| curl | 85,348 | 90,488 | 0.9432× | 0.95× | ✅ |
| dns | 230,034 | 233,296 | 0.9860× | 0.99× | ✅ |

Geometric mean 0.8539×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
