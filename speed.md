# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8956685` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,226 | 15,213 | 0.9351× | 0.93× | ✅ |
| echo-1k | 17,339 | 18,162 | 0.9547× | 0.95× | ✅ |
| json | 22,326 | 23,158 | 0.9641× | 0.96× | ✅ |
| service-health | 16,824 | 17,777 | 0.9464× | 0.94× | ✅ |
| websocket | 6,683 | 8,188 | 0.8162× | 0.83× | ✅ |
| timer | 202,119 | 313,491 | 0.6447× | 0.65× | ✅ |
| app | 126,514 | 135,910 | 0.9309× | 0.93× | ✅ |
| http2 | 19,995 | 20,467 | 0.9770× | 0.98× | ✅ |
| table | 196,322 | 201,865 | 0.9725× | 0.97× | ✅ |
| channel | 140,012 | 154,654 | 0.9053× | 0.91× | ✅ |
| task | 32,109 | 33,265 | 0.9652× | 0.97× | ✅ |
| mysql | 49,864 | 51,061 | 0.9766× | 0.98× | ✅ |
| http-client | 34,053 | 35,804 | 0.9511× | 0.95× | ✅ |
| client | 18,818 | 19,756 | 0.9525× | 0.94× | ✅ |
| socket | 19,795 | 21,067 | 0.9396× | 0.94× | ✅ |
| gzip | 196,192 | 197,079 | 0.9955× | 1.0× | ✅ |
| websocket-deflate | 181,441 | 181,568 | 0.9993× | 1.0× | ✅ |
| tcp-length | 5,182 | 4,762 | 1.0881× | 1.1× | ➖ |
| tcp-eof | 5,713 | 10,231 | 0.5583× | 0.57× | ✅ |
| udp | 7,961 | 7,986 | 0.9968× | 1.0× | ✅ |
| connect | 18,392 | 18,703 | 0.9833× | 0.99× | ✅ |
| https | 109,130 | 108,810 | 1.0029× | 1.01× | ➖ |
| wss | 104,599 | 104,800 | 0.9981× | 1.0× | ✅ |
| atomic | 50,017 | 51,316 | 0.9747× | 0.98× | ✅ |
| process-mode | 16,638 | 16,956 | 0.9813× | 0.98× | ✅ |
| http2-concurrent | 375,181 | 408,882 | 0.9176× | 0.92× | ✅ |
| websocket-64k | 156,126 | 159,694 | 0.9777× | 0.98× | ✅ |
| websocket-fragmented | 12,926 | 16,331 | 0.7915× | 0.79× | ✅ |
| upload | 544,478 | 1,322,380 | 0.4117× | 0.42× | ✅ |
| static | 34,167 | 40,981 | 0.8337× | 0.85× | ✅ |
| pgsql | 23,499 | 24,595 | 0.9555× | 0.96× | ✅ |
| sqlite | 23,848 | 25,232 | 0.9452× | 0.95× | ✅ |
| curl | 85,679 | 90,370 | 0.9481× | 0.95× | ✅ |
| dns | 231,271 | 233,306 | 0.9913× | 1.0× | ✅ |

Geometric mean 0.9040×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
