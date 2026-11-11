# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `060afa8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,679 | 15,210 | 0.9651× | 0.95× | ✅ |
| echo-1k | 17,853 | 18,168 | 0.9826× | 0.96× | ✅ |
| json | 22,709 | 23,174 | 0.9799× | 0.97× | ✅ |
| service-health | 17,260 | 17,795 | 0.9700× | 0.96× | ✅ |
| websocket | 6,863 | 8,188 | 0.8383× | 0.81× | ✅ |
| timer | 202,196 | 313,382 | 0.6452× | 0.65× | ✅ |
| app | 127,118 | 135,961 | 0.9350× | 0.92× | ✅ |
| http2 | 20,242 | 20,464 | 0.9892× | 0.97× | ✅ |
| table | 197,632 | 202,907 | 0.9740× | 0.98× | ✅ |
| channel | 140,267 | 155,192 | 0.9038× | 0.89× | ✅ |
| task | 32,641 | 33,269 | 0.9811× | 0.96× | ✅ |
| mysql | 50,293 | 51,076 | 0.9847× | 0.97× | ✅ |
| http-client | 34,745 | 35,824 | 0.9699× | 0.94× | ✅ |
| client | 19,188 | 19,775 | 0.9703× | 0.95× | ✅ |
| socket | 20,665 | 21,090 | 0.9798× | 0.99× | ✅ |
| gzip | 197,025 | 197,107 | 0.9996× | 1.01× | ✅ |
| websocket-deflate | 181,658 | 181,564 | 1.0005× | 1.0× | ❌ |
| tcp-length | 5,364 | 4,746 | 1.1302× | 1.07× | ❌ |
| tcp-eof | 5,858 | 10,212 | 0.5737× | 0.55× | ✅ |
| udp | 7,525 | 7,329 | 1.0268× | 1.01× | ❌ |
| connect | 21,432 | 18,707 | 1.1457× | 1.09× | ❌ |
| https | 109,956 | 108,803 | 1.0106× | 1.01× | ❌ |
| wss | 105,150 | 104,794 | 1.0034× | 1.0× | ❌ |
| atomic | 53,111 | 51,330 | 1.0347× | 1.04× | ➖ |
| process-mode | 16,925 | 16,945 | 0.9988× | 0.98× | ✅ |
| http2-concurrent | 377,080 | 409,299 | 0.9213× | 0.93× | ✅ |
| websocket-64k | 156,308 | 159,698 | 0.9788× | 1.03× | ✅ |
| websocket-fragmented | 14,017 | 16,194 | 0.8656× | 0.88× | ✅ |
| upload | 674,722 | 1,322,467 | 0.5102× | 0.52× | ✅ |
| static | 36,592 | 41,092 | 0.8905× | 0.96× | ✅ |

Geometric mean 0.9262×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
