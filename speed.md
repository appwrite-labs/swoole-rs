# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `eab3d3c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,900 | 15,210 | 0.9139× | 0.92× | ✅ |
| echo-1k | 17,022 | 18,162 | 0.9372× | 0.94× | ✅ |
| json | 21,990 | 23,161 | 0.9494× | 0.96× | ✅ |
| service-health | 16,491 | 17,777 | 0.9277× | 0.93× | ✅ |
| websocket | 6,731 | 8,188 | 0.8221× | 0.82× | ✅ |
| timer | 202,222 | 313,623 | 0.6448× | 0.65× | ✅ |
| app | 125,937 | 135,918 | 0.9266× | 0.93× | ✅ |
| http2 | 19,177 | 20,464 | 0.9371× | 0.97× | ✅ |
| table | 195,703 | 201,865 | 0.9695× | 0.97× | ✅ |
| channel | 139,366 | 154,654 | 0.9011× | 0.9× | ✅ |
| task | 31,923 | 33,262 | 0.9597× | 0.96× | ✅ |
| mysql | 49,505 | 51,064 | 0.9695× | 0.97× | ✅ |
| http-client | 33,563 | 35,804 | 0.9374× | 0.94× | ✅ |
| client | 18,446 | 19,757 | 0.9337× | 0.94× | ✅ |
| socket | 19,731 | 21,067 | 0.9366× | 0.92× | ✅ |
| gzip | 195,997 | 197,083 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,491 | 181,572 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,762 | 1.0547× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,231 | 0.5439× | 0.54× | ✅ |
| udp | 8,054 | 7,986 | 1.0086× | 1.0× | ❌ |
| connect | 18,238 | 18,707 | 0.9749× | 0.98× | ✅ |
| https | 108,761 | 108,803 | 0.9996× | 1.0× | ✅ |
| wss | 104,606 | 104,794 | 0.9982× | 1.0× | ✅ |
| atomic | 49,654 | 51,312 | 0.9677× | 0.97× | ✅ |
| process-mode | 16,402 | 16,959 | 0.9671× | 0.97× | ✅ |
| http2-concurrent | 361,267 | 408,882 | 0.8835× | 0.91× | ✅ |
| websocket-64k | 156,173 | 159,691 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 13,083 | 16,328 | 0.8013× | 0.79× | ✅ |
| upload | 544,622 | 1,322,384 | 0.4118× | 0.42× | ✅ |
| static | 34,434 | 41,074 | 0.8383× | 0.84× | ✅ |
| pgsql | 23,188 | 24,598 | 0.9427× | 0.95× | ✅ |
| sqlite | 23,469 | 25,232 | 0.9301× | 0.94× | ✅ |
| curl | 85,304 | 90,325 | 0.9444× | 0.95× | ✅ |
| dns | 230,992 | 232,927 | 0.9917× | 1.0× | ✅ |

Geometric mean 0.8959×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
