<?php
// Supplemental public API witness: row reuse across the native snapshot batches.
function clear_require(bool $condition, string $message): void
{
    if (!$condition) {
        throw new RuntimeException($message);
    }
}
$counts = [1, 7, 8, 9, 15, 16, 17, 33];
$records = 0;
foreach ($counts as $count) {
    $table = new Swoole\Table(64, 1.0);
    $defaults = $filled = [];
    for ($i = 0; $i < $count; ++$i) {
        $name = 'field' . $i;
        $kind = 1 + $i % 3;
        clear_require($table->column($name, $kind, 8), 'column');
        $defaults[$name] = $kind === 1 ? 0 : ($kind === 2 ? 0.0 : '');
        $filled[$name] = $kind === 1 ? 42 : ($kind === 2 ? 2.5 : "ab\0cd");
    }
    clear_require($table->create(), 'create');
    $last = 'field' . ($count - 1);
    $partial = $defaults;
    $partial[$last] = $filled[$last];
    for ($i = 0; $i < 16; ++$i) {
        $key = "batch\0" . $i;
        clear_require($table->set($key, $filled), 'fill');
        clear_require($table->get($key) === $filled, 'filled values');
    }
    for ($i = 0; $i < 16; ++$i) {
        $key = "batch\0" . $i;
        clear_require($table->del($key), 'delete filled row');
        clear_require($table->set($key, [$last => $filled[$last]]), 'partial recreate');
        clear_require($table->get($key) === $partial, 'missing fields clear across batches');
        clear_require($table->del($key), 'delete partial row');
        clear_require($table->set($key, []), 'empty recreate');
        $row = $table->get($key);
        clear_require($row === $defaults, 'all fields clear across batches');
        foreach ($row as $value) {
            if (is_float($value)) {
                clear_require(pack('d', $value) === pack('d', 0.0), 'positive float zero');
            }
        }
        clear_require($table->del($key), 'delete cleared row');
        ++$records;
    }
    if ($count === 33) {
        $key = "child\0clear";
        $child = new Swoole\Process(static function () use ($table, $key, $filled, $defaults): void {
            clear_require($table->set($key, $filled), 'child fill');
            clear_require($table->del($key), 'child delete');
            clear_require($table->set($key, []), 'child clear');
            clear_require($table->get($key) === $defaults, 'child defaults');
        }, false, SOCK_STREAM);
        $pid = $child->start();
        clear_require(is_int($pid) && $pid > 0, 'child start');
        $status = Swoole\Process::wait();
        clear_require(is_array($status) && $status['pid'] === $pid && $status['code'] === 0
            && $status['signal'] === 0, 'child exit');
        clear_require($table->get($key) === $defaults, 'parent observes cleared values');
        clear_require($table->del($key), 'parent delete');
        unset($child);
    }
    clear_require($table->count() === 0, 'empty accounting');
    clear_require($table->destroy(), 'destroy');
}
echo json_encode(['schemas' => $counts, 'reused_records' => $records,
    'recreations' => 2 * $records, 'forks' => 1, 'positive_float_zero' => true], JSON_THROW_ON_ERROR), PHP_EOL;
