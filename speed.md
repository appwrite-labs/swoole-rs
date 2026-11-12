# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3ca6fa6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,938 | 15,216 | 0.9160× | 0.92× | ✅ |
| echo-1k | 17,042 | 18,161 | 0.9384× | 0.94× | ✅ |
| json | 22,036 | 23,158 | 0.9516× | 0.95× | ✅ |
| service-health | 16,538 | 17,771 | 0.9306× | 0.93× | ✅ |
| websocket | 6,628 | 8,191 | 0.8091× | 0.82× | ✅ |
| timer | 203,345 | 313,476 | 0.6487× | 0.65× | ✅ |
| app | 125,904 | 135,996 | 0.9258× | 0.93× | ✅ |
| http2 | 18,699 | 20,488 | 0.9127× | 0.93× | ✅ |
| table | 195,293 | 201,916 | 0.9672× | 0.97× | ✅ |
| channel | 138,896 | 154,724 | 0.8977× | 0.9× | ✅ |
| task | 32,331 | 33,307 | 0.9707× | 0.96× | ✅ |
| mysql | 49,601 | 51,008 | 0.9724× | 0.97× | ✅ |
| http-client | 33,746 | 35,817 | 0.9422× | 0.94× | ✅ |
| client | 18,495 | 19,757 | 0.9361× | 0.94× | ✅ |
| socket | 19,614 | 21,062 | 0.9312× | 0.92× | ✅ |
| gzip | 196,017 | 197,084 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,395 | 181,580 | 0.9990× | 1.0× | ✅ |
| tcp-length | 4,839 | 4,762 | 1.0161× | 1.05× | ➖ |
| tcp-eof | 5,387 | 10,231 | 0.5265× | 0.54× | ✅ |
| udp | 7,623 | 7,986 | 0.9545× | 0.95× | ✅ |
| connect | 18,432 | 18,714 | 0.9849× | 0.98× | ✅ |
| https | 108,798 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,518 | 104,794 | 0.9974× | 1.0× | ✅ |
| atomic | 49,696 | 51,316 | 0.9684× | 0.97× | ✅ |
| process-mode | 16,775 | 16,959 | 0.9892× | 0.99× | ✅ |
| http2-concurrent | 356,016 | 408,781 | 0.8709× | 0.88× | ✅ |
| websocket-64k | 156,078 | 159,695 | 0.9774× | 0.98× | ✅ |
| websocket-fragmented | 12,862 | 16,331 | 0.7876× | 0.79× | ✅ |
| upload | 478,460 | 1,322,384 | 0.3618× | 0.37× | ✅ |
| static | 29,206 | 41,010 | 0.7122× | 0.72× | ✅ |
| pgsql | 23,200 | 24,596 | 0.9432× | 0.95× | ✅ |
| sqlite | 23,484 | 25,232 | 0.9307× | 0.94× | ✅ |
| curl | 84,714 | 90,465 | 0.9364× | 0.94× | ✅ |
| dns | 230,246 | 232,862 | 0.9888× | 0.99× | ✅ |

Geometric mean 0.8842×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
