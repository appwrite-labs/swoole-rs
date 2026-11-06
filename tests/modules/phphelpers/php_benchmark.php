<?php
// Actual optimized-extension preparation. --check reads no elapsed clock.
function ensure(bool $condition, string $message): void {
    if (!$condition) throw new RuntimeException($message);
}
foreach (['swoole_typed_array', 'm26_bool_call', 'm26_owned_callable'] as $function) {
    if (!function_exists($function)) { fwrite(STDERR, "UNSUPPORTED: $function required\n"); exit(2); }
}
if (PHP_INT_SIZE !== 8) { fwrite(STDERR, "UNSUPPORTED: 64-bit PHP benchmark driver required\n"); exit(2); }
class M26BenchCallable { public function __invoke(): void {} }
function m26_bench_factory(): object { return new M26BenchCallable(); }
function once(string $name, int $seed): int {
    switch ($name) {
    case 'typed_create':
        $array = swoole_typed_array('<int>', [1, 2, $seed]);
        return array_sum($array) + count($array);
    case 'typed_normalize':
        $array = swoole_typed_array(' < int > ', [1, $seed]);
        return array_sum($array) + count($array);
    case 'typed_nested':
        $array = swoole_typed_array('<<int>>');
        $array[] = swoole_typed_array('<int>', [1, $seed]);
        return array_sum($array[0]) + count($array);
    case 'typed_map_assign':
        $array = swoole_typed_array('<string,int>', ['a' => 1]);
        $array['b'] = $seed;
        return array_sum($array) + count($array);
    case 'typed_list_append':
        $array = swoole_typed_array('<int>', [1, 2]);
        $array[] = $seed;
        return array_sum($array) + count($array);
    case 'typed_compound':
        $array = swoole_typed_array('<int>', [1, 2]);
        $array[0] += $seed;
        return array_sum($array);
    case 'typed_cow_push':
        $original = swoole_typed_array('<int>', [1, 2]);
        $copy = $original;
        array_push($copy, $seed);
        ensure($original === [1, 2], 'COW source changed');
        return array_sum($copy) + count($original);
    case 'typed_splice':
        $array = swoole_typed_array('<int>', [1, 2, 3, 4]);
        $removed = array_splice($array, 1, 1, [$seed]);
        return array_sum($array) + array_sum($removed);
    case 'alias_sum':
        $array = [1, 2, $seed];
        return $array->sum();
    case 'wrapper_map':
        return array_sum(swoole_array_map([1, 2, 3], static fn(int $value): int => $value + $seed));
    case 'json_array':
        $array = swoole_str_json_decode('{"a":[1,2,3],"b":4}', 512, JSON_THROW_ON_ERROR);
        return array_sum($array['a']) + $array['b'];
    case 'pcre_single':
        $matches = swoole_str_match('A1 B22 C333', '/([A-Z])(\d+)/');
        return count($matches) + strlen($matches[0]);
    case 'bool_cached_direct':
        $result = m26_bool_call(static fn(): int => $seed + 9, false, true);
        ensure($result['success'] && $result['value_defined'], 'bool call did not publish result');
        return $result['value'];
    case 'named_strlen':
        return m26_named_call('strlen', str_repeat('a', 16 + ($seed & 7)));
    case 'callable_release':
        return (int) m26_owned_callable('m26_bench_factory');
    default: throw new RuntimeException('Unknown workload');
    }
}
$workloads = ['typed_create', 'typed_normalize', 'typed_nested', 'typed_map_assign',
    'typed_list_append', 'typed_compound', 'typed_cow_push', 'typed_splice', 'alias_sum',
    'wrapper_map', 'json_array', 'pcre_single', 'bool_cached_direct', 'named_strlen',
    'callable_release'];
if (($argv[1] ?? '') === '--check' && count($argv) === 2) {
    $outputs = [];
    foreach ($workloads as $name) foreach ([0, 1, 7] as $seed) $outputs[$name][] = once($name, $seed);
    echo json_encode(['workloads' => count($workloads), 'checks' => count($workloads) * 3,
        'outputs' => $outputs, 'elapsed_ns' => 0], JSON_THROW_ON_ERROR), "\n";
    exit(0);
}
if (count($argv) !== 5 || $argv[1] !== '--measure' || !in_array($argv[2], $workloads, true)
    || !preg_match('/^[0-9]+$/D', $argv[3]) || (int)$argv[3] < 1 || (int)$argv[3] > 100000000 || $argv[4] === '') {
    fwrite(STDERR, "Use --check, or --measure WORKLOAD ITERATIONS ACKNOWLEDGED_HOST_LEASE only after actual exclusive acknowledgment.\n");
    exit(2);
}
$name = $argv[2];
$iterations = (int)$argv[3];
once($name, 0);
$checksum = 0;
$begin = hrtime(true);
for ($i = 0; $i < $iterations; ++$i) $checksum ^= once($name, $i & 7);
$elapsed = hrtime(true) - $begin;
echo json_encode(['workload' => $name, 'iterations' => $iterations,
    'checksum' => $checksum, 'elapsed_ns' => $elapsed], JSON_THROW_ON_ERROR), "\n";
