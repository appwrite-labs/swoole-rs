# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `46edfed` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,080 | 23,173 | 0.9528× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,930 | 313,718 | 0.6437× | 0.65× | ✅ |
| app | 127,639 | 135,913 | 0.9391× | 0.93× | ✅ |
| http2 | 18,960 | 20,495 | 0.9251× | 0.92× | ✅ |
| table | 194,306 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,086 | 33,262 | 0.9647× | 0.96× | ✅ |
| mysql | 50,308 | 51,085 | 0.9848× | 0.97× | ✅ |
| http-client | 33,660 | 35,796 | 0.9403× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,533 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,813 | 0.9770× | 0.98× | ✅ |
| wss | 101,586 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,021 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,363 | 409,058 | 0.8907× | 0.89× | ✅ |
| websocket-64k | 90,005 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7282× | 0.72× | ✅ |
| upload | 416,523 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,877 | 40,999 | 0.6312× | 0.64× | ✅ |
| client | 18,623 | 19,730 | 0.9439× | 0.93× | ✅ |
| socket | 19,860 | 21,046 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,827 | 24,663 | 0.9256× | 0.92× | ✅ |
| sqlite | 23,527 | 25,253 | 0.9316× | 0.93× | ✅ |
| curl | 86,131 | 90,422 | 0.9525× | 0.95× | ✅ |
| dns | 229,863 | 233,309 | 0.9852× | 0.99× | ✅ |
| file | 109,092 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
