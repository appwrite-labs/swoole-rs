# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a6b4d0e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,339 | 15,213 | 0.9425× | 0.93× | ✅ |
| echo-1k | 17,457 | 18,165 | 0.9610× | 0.95× | ✅ |
| json | 22,433 | 23,161 | 0.9686× | 0.96× | ✅ |
| service-health | 16,931 | 17,777 | 0.9524× | 0.94× | ✅ |
| websocket | 6,732 | 8,188 | 0.8222× | 0.83× | ✅ |
| timer | 202,255 | 313,780 | 0.6446× | 0.65× | ✅ |
| app | 126,563 | 135,817 | 0.9319× | 0.93× | ✅ |
| http2 | 20,131 | 20,464 | 0.9837× | 0.98× | ✅ |
| table | 196,154 | 201,865 | 0.9717× | 0.97× | ✅ |
| channel | 139,359 | 154,654 | 0.9011× | 0.91× | ✅ |
| task | 32,277 | 33,268 | 0.9702× | 0.97× | ✅ |
| mysql | 49,965 | 51,052 | 0.9787× | 0.98× | ✅ |
| http-client | 34,153 | 35,805 | 0.9538× | 0.95× | ✅ |
| client | 18,912 | 19,754 | 0.9574× | 0.95× | ✅ |
| socket | 19,888 | 21,067 | 0.9440× | 0.94× | ✅ |
| gzip | 196,315 | 197,083 | 0.9961× | 1.0× | ✅ |
| websocket-deflate | 181,497 | 181,568 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,214 | 4,762 | 1.0948× | 1.1× | ➖ |
| tcp-eof | 5,745 | 10,228 | 0.5617× | 0.57× | ✅ |
| udp | 7,975 | 7,986 | 0.9986× | 1.0× | ✅ |
| connect | 18,560 | 18,707 | 0.9921× | 0.99× | ✅ |
| https | 109,280 | 108,810 | 1.0043× | 1.01× | ➖ |
| wss | 104,678 | 104,800 | 0.9988× | 1.0× | ✅ |
| atomic | 50,101 | 51,316 | 0.9763× | 0.98× | ✅ |
| process-mode | 16,743 | 16,956 | 0.9874× | 0.98× | ✅ |
| http2-concurrent | 377,443 | 409,091 | 0.9226× | 0.92× | ✅ |
| websocket-64k | 156,186 | 159,697 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 12,996 | 16,327 | 0.7959× | 0.79× | ✅ |
| upload | 544,569 | 1,322,382 | 0.4118× | 0.42× | ✅ |
| static | 34,308 | 40,981 | 0.8372× | 0.85× | ✅ |
| pgsql | 23,728 | 24,595 | 0.9648× | 0.96× | ✅ |
| sqlite | 23,962 | 25,232 | 0.9497× | 0.95× | ✅ |
| curl | 85,770 | 90,507 | 0.9477× | 0.95× | ✅ |
| dns | 231,555 | 232,541 | 0.9958× | 1.0× | ✅ |

Geometric mean 0.9073×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
