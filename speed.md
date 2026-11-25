# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `551b40c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,766 | 15,210 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,094 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,344 | 17,786 | 0.9190× | 0.92× | ✅ |
| websocket | 6,723 | 8,185 | 0.8213× | 0.81× | ✅ |
| timer | 202,460 | 314,697 | 0.6433× | 0.65× | ✅ |
| app | 127,611 | 135,963 | 0.9386× | 0.93× | ✅ |
| http2 | 19,083 | 20,499 | 0.9309× | 0.92× | ✅ |
| table | 194,313 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,369 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,082 | 33,268 | 0.9644× | 0.96× | ✅ |
| mysql | 50,331 | 51,085 | 0.9852× | 0.97× | ✅ |
| http-client | 33,719 | 35,793 | 0.9420× | 0.93× | ✅ |
| gzip | 194,296 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,303 | 181,573 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,618 | 7,987 | 0.9538× | 0.95× | ✅ |
| connect | 17,540 | 18,734 | 0.9363× | 0.93× | ✅ |
| https | 106,308 | 108,806 | 0.9770× | 0.98× | ✅ |
| wss | 101,583 | 104,800 | 0.9693× | 0.97× | ✅ |
| atomic | 50,199 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,432 | 17,024 | 0.9065× | 0.89× | ✅ |
| http2-concurrent | 366,113 | 409,053 | 0.8950× | 0.89× | ✅ |
| websocket-64k | 90,006 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,925 | 16,368 | 0.7285× | 0.72× | ✅ |
| upload | 416,514 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,850 | 40,999 | 0.6305× | 0.64× | ✅ |
| client | 18,635 | 19,730 | 0.9445× | 0.93× | ✅ |
| socket | 19,871 | 21,045 | 0.9442× | 0.93× | ✅ |
| pgsql | 22,817 | 24,663 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,141 | 90,415 | 0.9527× | 0.95× | ✅ |
| dns | 230,137 | 232,841 | 0.9884× | 0.99× | ✅ |
| file | 108,940 | 131,207 | 0.8303× | 0.83× | ✅ |

Geometric mean 0.8540×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
