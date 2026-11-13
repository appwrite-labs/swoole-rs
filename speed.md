# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `268f63c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,915 | 15,214 | 0.9147× | 0.92× | ✅ |
| echo-1k | 17,039 | 18,165 | 0.9380× | 0.94× | ✅ |
| json | 22,007 | 23,161 | 0.9502× | 0.95× | ✅ |
| service-health | 16,505 | 17,777 | 0.9285× | 0.93× | ✅ |
| websocket | 6,525 | 8,188 | 0.7969× | 0.81× | ✅ |
| timer | 203,169 | 313,568 | 0.6479× | 0.65× | ✅ |
| app | 125,366 | 135,947 | 0.9222× | 0.93× | ✅ |
| http2 | 18,569 | 20,468 | 0.9072× | 0.92× | ✅ |
| table | 195,483 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,424 | 154,654 | 0.8886× | 0.89× | ✅ |
| task | 31,992 | 33,265 | 0.9617× | 0.96× | ✅ |
| mysql | 49,519 | 51,061 | 0.9698× | 0.97× | ✅ |
| http-client | 33,668 | 35,809 | 0.9402× | 0.94× | ✅ |
| client | 18,461 | 19,757 | 0.9344× | 0.94× | ✅ |
| socket | 19,588 | 21,064 | 0.9299× | 0.92× | ✅ |
| gzip | 194,138 | 197,083 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,313 | 181,572 | 0.9876× | 0.99× | ✅ |
| tcp-length | 4,739 | 4,765 | 0.9945× | 1.01× | ✅ |
| tcp-eof | 5,262 | 10,228 | 0.5145× | 0.52× | ✅ |
| udp | 7,536 | 7,986 | 0.9436× | 0.95× | ✅ |
| connect | 17,891 | 18,707 | 0.9564× | 0.98× | ✅ |
| https | 108,195 | 108,803 | 0.9944× | 1.0× | ✅ |
| wss | 103,804 | 104,794 | 0.9906× | 1.0× | ✅ |
| atomic | 49,686 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 15,126 | 17,025 | 0.8884× | 0.94× | ✅ |
| http2-concurrent | 356,781 | 408,877 | 0.8726× | 0.88× | ✅ |
| websocket-64k | 155,968 | 159,692 | 0.9767× | 0.98× | ✅ |
| websocket-fragmented | 12,753 | 16,327 | 0.7811× | 0.79× | ✅ |
| upload | 477,953 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 27,613 | 41,016 | 0.6732× | 0.72× | ✅ |
| pgsql | 23,205 | 24,594 | 0.9435× | 0.95× | ✅ |
| sqlite | 23,475 | 25,236 | 0.9302× | 0.94× | ✅ |
| curl | 85,710 | 90,473 | 0.9474× | 0.94× | ✅ |
| dns | 230,825 | 232,631 | 0.9922× | 0.99× | ✅ |

Geometric mean 0.8755×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
