# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2679242` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,313 | 15,213 | 0.9408× | 0.93× | ✅ |
| echo-1k | 17,438 | 18,165 | 0.9600× | 0.95× | ✅ |
| json | 22,398 | 23,170 | 0.9667× | 0.96× | ✅ |
| service-health | 16,897 | 17,786 | 0.9500× | 0.94× | ✅ |
| websocket | 6,724 | 8,191 | 0.8209× | 0.83× | ✅ |
| timer | 202,216 | 313,797 | 0.6444× | 0.65× | ✅ |
| app | 126,775 | 135,886 | 0.9330× | 0.93× | ✅ |
| http2 | 20,087 | 20,464 | 0.9816× | 0.98× | ✅ |
| table | 195,971 | 201,874 | 0.9708× | 0.97× | ✅ |
| channel | 139,596 | 154,664 | 0.9026× | 0.91× | ✅ |
| task | 32,220 | 33,266 | 0.9686× | 0.97× | ✅ |
| mysql | 50,022 | 51,055 | 0.9798× | 0.98× | ✅ |
| http-client | 34,255 | 35,826 | 0.9561× | 0.95× | ✅ |
| client | 18,935 | 19,766 | 0.9580× | 0.94× | ✅ |
| socket | 19,895 | 21,071 | 0.9442× | 0.94× | ✅ |
| gzip | 196,305 | 197,079 | 0.9961× | 1.0× | ✅ |
| websocket-deflate | 181,485 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,229 | 4,763 | 1.0978× | 1.1× | ➖ |
| tcp-eof | 5,802 | 10,231 | 0.5670× | 0.57× | ✅ |
| udp | 7,989 | 7,986 | 1.0004× | 1.0× | ❌ |
| connect | 18,538 | 18,707 | 0.9909× | 0.99× | ✅ |
| https | 109,410 | 108,803 | 1.0056× | 1.01× | ➖ |
| wss | 104,836 | 104,802 | 1.0003× | 1.0× | ❌ |
| atomic | 50,114 | 51,325 | 0.9764× | 0.98× | ✅ |
| process-mode | 16,751 | 16,959 | 0.9877× | 0.98× | ✅ |
| http2-concurrent | 376,024 | 409,074 | 0.9192× | 0.92× | ✅ |
| websocket-64k | 156,172 | 159,707 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 12,977 | 16,335 | 0.7944× | 0.79× | ✅ |
| upload | 544,631 | 1,322,382 | 0.4119× | 0.42× | ✅ |
| static | 34,522 | 40,981 | 0.8424× | 0.85× | ✅ |
| pgsql | 23,614 | 24,594 | 0.9602× | 0.96× | ✅ |
| sqlite | 23,937 | 25,232 | 0.9487× | 0.95× | ✅ |
| curl | 85,801 | 90,492 | 0.9482× | 0.95× | ✅ |
| dns | 231,159 | 232,841 | 0.9928× | 1.0× | ✅ |

Geometric mean 0.9073×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
