# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `04d1e4e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,933 | 15,213 | 0.9159× | 0.92× | ✅ |
| echo-1k | 17,036 | 18,165 | 0.9378× | 0.94× | ✅ |
| json | 22,040 | 23,161 | 0.9516× | 0.95× | ✅ |
| service-health | 16,535 | 17,777 | 0.9301× | 0.93× | ✅ |
| websocket | 6,575 | 8,191 | 0.8026× | 0.82× | ✅ |
| timer | 203,389 | 313,672 | 0.6484× | 0.65× | ✅ |
| app | 125,811 | 135,971 | 0.9253× | 0.93× | ✅ |
| http2 | 18,666 | 20,464 | 0.9121× | 0.93× | ✅ |
| table | 195,738 | 201,869 | 0.9696× | 0.97× | ✅ |
| channel | 138,889 | 154,651 | 0.8981× | 0.9× | ✅ |
| task | 32,157 | 33,265 | 0.9667× | 0.96× | ✅ |
| mysql | 49,607 | 51,061 | 0.9715× | 0.97× | ✅ |
| http-client | 33,710 | 35,808 | 0.9414× | 0.94× | ✅ |
| client | 18,488 | 19,757 | 0.9357× | 0.94× | ✅ |
| socket | 19,606 | 21,068 | 0.9306× | 0.92× | ✅ |
| gzip | 196,015 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,338 | 181,568 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,774 | 4,762 | 1.0024× | 1.05× | ➖ |
| tcp-eof | 5,322 | 10,231 | 0.5201× | 0.54× | ✅ |
| udp | 7,585 | 7,986 | 0.9497× | 0.95× | ✅ |
| connect | 18,362 | 18,707 | 0.9816× | 0.98× | ✅ |
| https | 108,796 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,468 | 104,794 | 0.9969× | 1.0× | ✅ |
| atomic | 49,685 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,690 | 16,963 | 0.9839× | 0.99× | ✅ |
| http2-concurrent | 359,240 | 408,882 | 0.8786× | 0.88× | ✅ |
| websocket-64k | 156,021 | 159,696 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,813 | 16,331 | 0.7846× | 0.79× | ✅ |
| upload | 478,441 | 1,322,382 | 0.3618× | 0.37× | ✅ |
| static | 29,185 | 40,981 | 0.7122× | 0.72× | ✅ |
| pgsql | 23,210 | 24,593 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,492 | 25,236 | 0.9309× | 0.94× | ✅ |
| curl | 84,601 | 90,350 | 0.9364× | 0.94× | ✅ |
| dns | 230,799 | 233,277 | 0.9894× | 0.99× | ✅ |

Geometric mean 0.8829×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
