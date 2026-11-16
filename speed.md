# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `94a6fef` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,760 | 15,252 | 0.9021× | 0.91× | ✅ |
| echo-1k | 16,792 | 18,204 | 0.9224× | 0.93× | ✅ |
| json | 21,911 | 23,200 | 0.9444× | 0.95× | ✅ |
| service-health | 16,345 | 17,816 | 0.9175× | 0.93× | ✅ |
| websocket | 6,532 | 8,230 | 0.7937× | 0.8× | ✅ |
| timer | 206,518 | 313,741 | 0.6582× | 0.65× | ✅ |
| app | 126,037 | 136,103 | 0.9260× | 0.93× | ✅ |
| http2 | 18,787 | 20,507 | 0.9161× | 0.92× | ✅ |
| table | 194,809 | 201,904 | 0.9649× | 0.97× | ✅ |
| channel | 137,832 | 155,248 | 0.8878× | 0.88× | ✅ |
| task | 32,472 | 33,265 | 0.9762× | 0.96× | ✅ |
| mysql | 49,541 | 51,091 | 0.9697× | 0.97× | ✅ |
| http-client | 33,399 | 35,842 | 0.9319× | 0.93× | ✅ |
| gzip | 193,864 | 197,118 | 0.9835× | 0.99× | ✅ |
| websocket-deflate | 178,620 | 181,611 | 0.9835× | 0.99× | ✅ |
| tcp-length | 4,550 | 4,804 | 0.9470× | 0.96× | ✅ |
| tcp-eof | 5,130 | 10,267 | 0.4996× | 0.5× | ✅ |
| udp | 7,465 | 8,025 | 0.9302× | 0.93× | ✅ |
| connect | 17,384 | 18,742 | 0.9275× | 0.93× | ✅ |
| https | 105,796 | 108,842 | 0.9720× | 0.98× | ✅ |
| wss | 100,781 | 104,833 | 0.9614× | 0.97× | ✅ |
| atomic | 49,856 | 51,355 | 0.9708× | 0.97× | ✅ |
| process-mode | 14,932 | 16,998 | 0.8785× | 0.88× | ✅ |
| http2-concurrent | 359,904 | 409,504 | 0.8789× | 0.88× | ✅ |
| websocket-64k | 89,788 | 159,736 | 0.5621× | 0.57× | ✅ |
| websocket-fragmented | 11,694 | 16,366 | 0.7145× | 0.72× | ✅ |
| upload | 413,906 | 1,322,426 | 0.3130× | 0.32× | ✅ |
| static | 26,745 | 40,981 | 0.6526× | 0.65× | ✅ |
| client | 18,429 | 19,796 | 0.9309× | 0.94× | ✅ |
| socket | 19,591 | 21,112 | 0.9280× | 0.92× | ✅ |
| pgsql | 22,925 | 24,594 | 0.9321× | 0.93× | ✅ |
| sqlite | 23,425 | 25,236 | 0.9282× | 0.93× | ✅ |
| curl | 84,589 | 90,346 | 0.9363× | 0.94× | ✅ |
| dns | 229,111 | 232,898 | 0.9837× | 0.99× | ✅ |

Geometric mean 0.8486×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
