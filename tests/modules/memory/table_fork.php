<?php
// Separate M12 lifetime witness; does not replace any original PHPT assertion.
function memory_require(bool $condition, string $message): void
{
    if (!$condition) {
        throw new RuntimeException($message);
    }
}

memory_require(class_exists(Swoole\Table::class) && class_exists(Swoole\Process::class), 'Table/Process required');
$shared = new Swoole\Table(64);
$shared->column('label', Swoole\Table::TYPE_STRING, 64);
$shared->column('value', Swoole\Table::TYPE_INT, 8);
memory_require($shared->create(), 'shared create');
$label = "parent\0binary";
memory_require($shared->set("shared\0key", ['label' => $label, 'value' => 0]), 'shared set');

for ($round = 0; $round < 4; ++$round) {
    $process = new Swoole\Process(static function () use ($shared, $round, $label): void {
        memory_require($shared->get("shared\0key", 'value') === $round, 'inherited integer lookup');
        memory_require($shared->get("shared\0key", 'label') === $label, 'inherited binary lookup');
        for ($iteration = 0; $iteration < 64; ++$iteration) {
            $temporary = new Swoole\Table(65);
            // Grow and drain the native-token index repeatedly after fork.
            for ($column = 0; $column < 24; ++$column) {
                memory_require($temporary->column('c' . $column, Swoole\Table::TYPE_INT, 8), 'column definition');
            }
            $temporary->column("bytes\0column", Swoole\Table::TYPE_STRING, 31);
            $temporary->column('fraction', Swoole\Table::TYPE_FLOAT);
            memory_require($temporary->create(), 'child create');
            $key = 'row' . $iteration . "\0binary";
            $bytes = "\0" . pack('N', $iteration) . "\xff";
            memory_require($temporary->set($key, ['c0' => $iteration, 'c23' => -$iteration,
                "bytes\0column" => $bytes, 'fraction' => 1.25]), 'child set');
            $row = $temporary->get($key);
            memory_require($row['c0'] === $iteration && $row['c23'] === -$iteration, 'grown column lookup');
            memory_require($row["bytes\0column"] === $bytes && $row['fraction'] === 1.25, 'typed row values');
            $seen = 0;
            foreach ($temporary as $found => $value) {
                memory_require($found === $key && $value === $row, 'child iteration');
                ++$seen;
            }
            memory_require($seen === 1, 'iterator cardinality');
            memory_require($temporary->del($key), 'child delete');
            memory_require($temporary->destroy(), 'child destroy');
            unset($temporary);
        }
        memory_require($shared->incr("shared\0key", 'value') === $round + 1, 'shared increment');
        memory_require($shared->set("shared\0key", ['label' => 'child' . $round . "\0binary"]), 'shared child write');
    }, false, SOCK_STREAM);
    $pid = $process->start();
    memory_require(is_int($pid) && $pid > 0, 'fork start');
    $status = Swoole\Process::wait();
    memory_require(is_array($status) && $status['pid'] === $pid && $status['code'] === 0 && $status['signal'] === 0, 'child exit');
    $label = 'child' . $round . "\0binary";
    memory_require($shared->get("shared\0key", 'value') === $round + 1, 'parent shared value');
    memory_require($shared->get("shared\0key", 'label') === $label, 'parent index after child teardown');
    unset($process);
}
memory_require($shared->destroy(), 'parent destroy');
echo json_encode(['fork_rounds' => 4, 'transient_tables' => 256,
    'columns_per_transient_table' => 26, 'shared_value' => 4,
    'shared_label_hex' => bin2hex($label)], JSON_THROW_ON_ERROR), PHP_EOL;
