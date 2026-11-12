# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4cdce3d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,916 | 15,214 | 0.9147× | 0.92× | ✅ |
| echo-1k | 17,025 | 18,162 | 0.9374× | 0.94× | ✅ |
| json | 22,013 | 23,161 | 0.9504× | 0.95× | ✅ |
| service-health | 16,515 | 17,773 | 0.9292× | 0.93× | ✅ |
| websocket | 6,573 | 8,188 | 0.8028× | 0.82× | ✅ |
| timer | 203,242 | 313,511 | 0.6483× | 0.65× | ✅ |
| app | 125,732 | 135,964 | 0.9247× | 0.93× | ✅ |
| http2 | 18,665 | 20,464 | 0.9121× | 0.93× | ✅ |
| table | 195,468 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 137,398 | 154,655 | 0.8884× | 0.9× | ✅ |
| task | 32,157 | 33,264 | 0.9667× | 0.96× | ✅ |
| mysql | 49,573 | 51,061 | 0.9709× | 0.97× | ✅ |
| http-client | 33,659 | 35,805 | 0.9401× | 0.94× | ✅ |
| client | 18,472 | 19,757 | 0.9350× | 0.94× | ✅ |
| socket | 19,583 | 21,067 | 0.9295× | 0.92× | ✅ |
| gzip | 194,113 | 197,079 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,379 | 181,572 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,788 | 4,762 | 1.0054× | 1.05× | ➖ |
| tcp-eof | 5,336 | 10,231 | 0.5215× | 0.54× | ✅ |
| udp | 7,576 | 7,986 | 0.9487× | 0.95× | ✅ |
| connect | 18,352 | 18,703 | 0.9812× | 0.98× | ✅ |
| https | 108,806 | 108,813 | 0.9999× | 1.0× | ✅ |
| wss | 104,465 | 104,797 | 0.9968× | 1.0× | ✅ |
| atomic | 49,675 | 51,316 | 0.9680× | 0.97× | ✅ |
| process-mode | 16,670 | 16,960 | 0.9829× | 0.99× | ✅ |
| http2-concurrent | 355,750 | 408,882 | 0.8701× | 0.88× | ✅ |
| websocket-64k | 156,023 | 159,695 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,812 | 16,327 | 0.7847× | 0.79× | ✅ |
| upload | 478,693 | 1,322,380 | 0.3620× | 0.37× | ✅ |
| static | 29,270 | 41,039 | 0.7132× | 0.72× | ✅ |
| pgsql | 23,204 | 24,595 | 0.9434× | 0.95× | ✅ |
| sqlite | 23,487 | 25,236 | 0.9307× | 0.94× | ✅ |
| curl | 84,756 | 90,298 | 0.9386× | 0.94× | ✅ |
| dns | 230,536 | 232,636 | 0.9910× | 0.99× | ✅ |

Geometric mean 0.8818×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
