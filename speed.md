# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `63dc609` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,084 | 23,170 | 0.9531× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,726 | 8,185 | 0.8217× | 0.81× | ✅ |
| timer | 202,042 | 313,891 | 0.6437× | 0.65× | ✅ |
| app | 127,625 | 135,903 | 0.9391× | 0.93× | ✅ |
| http2 | 18,964 | 20,496 | 0.9252× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 32,086 | 33,271 | 0.9644× | 0.96× | ✅ |
| mysql | 50,300 | 51,088 | 0.9846× | 0.97× | ✅ |
| http-client | 33,629 | 35,789 | 0.9396× | 0.93× | ✅ |
| gzip | 194,285 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,163 | 181,570 | 0.9867× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,597 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,024 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,466 | 409,638 | 0.8897× | 0.89× | ✅ |
| websocket-64k | 90,006 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,369 | 0.7284× | 0.72× | ✅ |
| upload | 416,527 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,854 | 41,005 | 0.6305× | 0.64× | ✅ |
| client | 18,621 | 19,729 | 0.9438× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,826 | 24,662 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,130 | 90,417 | 0.9526× | 0.95× | ✅ |
| dns | 230,019 | 232,799 | 0.9881× | 0.99× | ✅ |
| file | 109,096 | 130,840 | 0.8338× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
