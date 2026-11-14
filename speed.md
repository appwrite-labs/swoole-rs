# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a429d5a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,971 | 15,210 | 0.9185× | 0.92× | ✅ |
| echo-1k | 17,055 | 18,165 | 0.9389× | 0.94× | ✅ |
| json | 22,100 | 23,161 | 0.9542× | 0.95× | ✅ |
| service-health | 16,541 | 17,777 | 0.9305× | 0.93× | ✅ |
| websocket | 6,633 | 8,191 | 0.8097× | 0.8× | ✅ |
| timer | 206,089 | 313,590 | 0.6572× | 0.65× | ✅ |
| app | 125,516 | 135,915 | 0.9235× | 0.93× | ✅ |
| http2 | 18,672 | 20,464 | 0.9125× | 0.91× | ✅ |
| table | 196,004 | 201,865 | 0.9710× | 0.97× | ✅ |
| channel | 136,695 | 155,170 | 0.8809× | 0.89× | ✅ |
| task | 32,493 | 33,264 | 0.9768× | 0.96× | ✅ |
| mysql | 49,672 | 51,053 | 0.9729× | 0.97× | ✅ |
| http-client | 33,896 | 35,803 | 0.9467× | 0.94× | ✅ |
| client | 18,589 | 19,757 | 0.9409× | 0.94× | ✅ |
| socket | 19,706 | 21,073 | 0.9352× | 0.92× | ✅ |
| gzip | 194,162 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,094 | 181,572 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9906× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,543 | 7,987 | 0.9444× | 0.93× | ✅ |
| connect | 17,745 | 18,707 | 0.9486× | 0.94× | ✅ |
| https | 108,213 | 108,810 | 0.9945× | 1.0× | ✅ |
| wss | 103,851 | 104,800 | 0.9909× | 0.99× | ✅ |
| atomic | 49,694 | 51,316 | 0.9684× | 0.97× | ✅ |
| process-mode | 15,090 | 17,025 | 0.8864× | 0.88× | ✅ |
| http2-concurrent | 361,068 | 409,072 | 0.8827× | 0.88× | ✅ |
| websocket-64k | 89,960 | 159,695 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,871 | 16,327 | 0.7270× | 0.74× | ✅ |
| upload | 413,437 | 1,322,392 | 0.3126× | 0.32× | ✅ |
| static | 27,261 | 40,987 | 0.6651× | 0.66× | ✅ |
| pgsql | 23,392 | 24,595 | 0.9511× | 0.95× | ✅ |
| sqlite | 23,562 | 25,236 | 0.9337× | 0.93× | ✅ |
| curl | 86,142 | 90,350 | 0.9534× | 0.94× | ✅ |
| dns | 231,736 | 232,722 | 0.9958× | 0.99× | ✅ |

Geometric mean 0.8580×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
