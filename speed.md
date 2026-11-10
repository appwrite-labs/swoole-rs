# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `bfd2f1a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,372 | 15,209 | 0.9450× | 0.91× | ✅ |
| echo-1k | 17,369 | 18,164 | 0.9562× | 0.92× | ✅ |
| json | 22,392 | 23,176 | 0.9662× | 0.94× | ✅ |
| service-health | 16,949 | 17,783 | 0.9531× | 0.93× | ✅ |
| websocket | 6,561 | 8,191 | 0.8010× | 0.81× | ✅ |
| timer | 209,241 | 313,408 | 0.6676× | 0.67× | ✅ |
| app | 125,820 | 135,976 | 0.9253× | 0.92× | ✅ |
| http2 | 19,806 | 20,485 | 0.9668× | 0.95× | ✅ |
| table | 197,907 | 201,934 | 0.9801× | 0.98× | ✅ |
| channel | 139,770 | 155,262 | 0.9002× | 0.89× | ✅ |
| task | 31,878 | 33,315 | 0.9569× | 0.94× | ✅ |
| mysql | 49,571 | 51,076 | 0.9705× | 0.97× | ✅ |
| http-client | 33,719 | 35,823 | 0.9413× | 0.95× | ✅ |
| client | 18,356 | 19,069 | 0.9626× | 0.95× | ✅ |
| socket | 19,896 | 18,332 | 1.0853× | 0.99× | ❌ |
| gzip | 200,801 | 197,093 | 1.0188× | 1.02× | ➖ |
| websocket-deflate | 181,365 | 181,568 | 0.9989× | 1.0× | ✅ |
| tcp-length | 5,054 | 4,746 | 1.0649× | 1.02× | ❌ |
| tcp-eof | 5,592 | 10,212 | 0.5476× | 0.55× | ✅ |
| udp | 7,999 | 7,316 | 1.0934× | 1.09× | ❌ |
| https | 109,155 | 108,814 | 1.0031× | 1.04× | ➖ |
| wss | 104,255 | 104,805 | 0.9948× | 1.04× | ✅ |
| atomic | 52,339 | 51,334 | 1.0196× | 1.02× | ➖ |
| process-mode | 16,514 | 16,949 | 0.9743× | 0.95× | ✅ |
| http2-concurrent | 1,475,751 | 409,084 | 3.6074× | 1.31× | ❌ |
| websocket-64k | 156,017 | 159,692 | 0.9770× | 0.62× | ✅ |
| websocket-fragmented | 13,547 | 16,194 | 0.8366× | 0.78× | ✅ |
| upload | 1,780,599 | 1,322,491 | 1.3464× | 1.05× | ❌ |
| static | 41,386 | 41,039 | 1.0084× | 1.03× | ➖ |

Geometric mean 0.9934×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
