# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `be1c827` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,105 | 15,211 | 0.9273× | 0.93× | ✅ |
| echo-1k | 17,002 | 18,151 | 0.9367× | 0.94× | ✅ |
| json | 22,097 | 23,164 | 0.9539× | 0.96× | ✅ |
| service-health | 16,724 | 17,795 | 0.9398× | 0.94× | ✅ |
| websocket | 6,714 | 8,191 | 0.8197× | 0.82× | ✅ |
| timer | 221,504 | 313,288 | 0.7070× | 0.72× | ✅ |
| app | 126,363 | 136,053 | 0.9288× | 0.93× | ✅ |
| http2 | 19,375 | 20,372 | 0.9510× | 0.95× | ✅ |
| table | 198,486 | 201,733 | 0.9839× | 0.99× | ✅ |
| channel | 137,117 | 155,155 | 0.8837× | 0.89× | ✅ |
| task | 31,582 | 33,181 | 0.9518× | 0.96× | ✅ |
| mysql | 49,790 | 51,091 | 0.9745× | 0.98× | ✅ |

Geometric mean 0.9098×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
