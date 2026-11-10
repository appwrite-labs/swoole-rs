# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `811c96f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,338 | 15,214 | 0.9425× | 0.91× | ✅ |
| echo-1k | 17,314 | 18,166 | 0.9531× | 0.92× | ✅ |
| json | 22,345 | 23,180 | 0.9640× | 0.94× | ✅ |
| service-health | 16,910 | 17,790 | 0.9506× | 0.93× | ✅ |
| websocket | 6,567 | 8,188 | 0.8021× | 0.81× | ✅ |
| timer | 207,925 | 313,270 | 0.6637× | 0.68× | ✅ |
| app | 125,690 | 135,963 | 0.9244× | 0.92× | ✅ |
| http2 | 19,561 | 20,387 | 0.9595× | 0.95× | ✅ |
| table | 197,172 | 203,101 | 0.9708× | 0.98× | ✅ |
| channel | 137,350 | 155,089 | 0.8856× | 0.89× | ✅ |
| task | 31,832 | 33,309 | 0.9556× | 0.94× | ✅ |
| mysql | 49,505 | 51,084 | 0.9691× | 0.97× | ✅ |
| tcp-length | 5,054 | 4,744 | 1.0654× | 1.02× | ❌ |
| tcp-eof | 5,592 | 10,212 | 0.5476× | 0.57× | ✅ |
| udp | 7,868 | 7,388 | 1.0650× | 1.09× | ➖ |

Geometric mean 0.8959×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
