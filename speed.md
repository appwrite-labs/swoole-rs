# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0a6522b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,820 | 15,213 | 0.9085× | 0.91× | ✅ |
| echo-1k | 16,853 | 18,162 | 0.9279× | 0.93× | ✅ |
| json | 21,957 | 23,161 | 0.9480× | 0.95× | ✅ |
| service-health | 16,404 | 17,777 | 0.9228× | 0.93× | ✅ |
| websocket | 6,510 | 8,188 | 0.7951× | 0.8× | ✅ |
| timer | 206,392 | 313,569 | 0.6582× | 0.65× | ✅ |
| app | 125,871 | 135,880 | 0.9263× | 0.93× | ✅ |
| http2 | 18,846 | 20,468 | 0.9208× | 0.92× | ✅ |
| table | 194,674 | 201,865 | 0.9644× | 0.97× | ✅ |
| channel | 138,156 | 155,170 | 0.8903× | 0.88× | ✅ |
| task | 32,466 | 33,261 | 0.9761× | 0.96× | ✅ |
| mysql | 49,580 | 51,032 | 0.9716× | 0.97× | ✅ |
| http-client | 33,447 | 35,803 | 0.9342× | 0.93× | ✅ |
| gzip | 193,939 | 197,079 | 0.9841× | 0.99× | ✅ |
| websocket-deflate | 178,944 | 181,568 | 0.9855× | 0.99× | ✅ |
| tcp-length | 4,526 | 4,762 | 0.9504× | 0.96× | ✅ |
| tcp-eof | 5,114 | 10,231 | 0.4998× | 0.5× | ✅ |
| udp | 7,412 | 7,986 | 0.9281× | 0.93× | ✅ |
| connect | 17,345 | 18,707 | 0.9272× | 0.93× | ✅ |
| https | 105,870 | 108,810 | 0.9730× | 0.98× | ✅ |
| wss | 100,769 | 104,794 | 0.9616× | 0.97× | ✅ |
| atomic | 49,916 | 51,316 | 0.9727× | 0.97× | ✅ |
| process-mode | 14,956 | 16,957 | 0.8820× | 0.88× | ✅ |
| http2-concurrent | 360,636 | 409,521 | 0.8806× | 0.88× | ✅ |
| websocket-64k | 89,759 | 159,695 | 0.5621× | 0.57× | ✅ |
| websocket-fragmented | 11,671 | 16,327 | 0.7148× | 0.72× | ✅ |
| upload | 414,030 | 1,322,389 | 0.3131× | 0.32× | ✅ |
| static | 26,562 | 40,987 | 0.6481× | 0.65× | ✅ |
| client | 18,480 | 19,753 | 0.9355× | 0.94× | ✅ |
| socket | 19,633 | 21,072 | 0.9317× | 0.92× | ✅ |
| pgsql | 22,643 | 24,595 | 0.9206× | 0.93× | ✅ |
| sqlite | 23,462 | 25,236 | 0.9297× | 0.93× | ✅ |
| curl | 84,745 | 90,481 | 0.9366× | 0.94× | ✅ |
| dns | 229,878 | 233,031 | 0.9865× | 0.99× | ✅ |

Geometric mean 0.8497×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
