<?php
// New module witness, separate from unchanged original PHPT assertions.
function require_state(bool $condition, string $message): void {
    if (!$condition) {
        throw new RuntimeException($message);
    }
}
$result = ['memory_sizes' => []];
foreach ([64, 65, 128] as $requested) {
    $table = new Swoole\Table($requested, 0.2);
    require_state($table->getMemorySize() === 0, 'memory size before create');
    require_state($table->column('integer', Swoole\Table::TYPE_INT), 'integer column');
    require_state($table->column('string', Swoole\Table::TYPE_STRING, 5), 'string column');
    require_state($table->column('double', Swoole\Table::TYPE_FLOAT), 'double column');
    require_state($table->create(), 'create');
    require_state($table->getMemorySize() > 0, 'positive memory size');
    require_state($table->memorySize === $table->getMemorySize(), 'memorySize property');
    $result['memory_sizes'][] = [$requested, $table->getSize(), $table->getMemorySize()];
    require_state($table->destroy(), 'destroy');
}
$table = new Swoole\Table(64, 0.2);
require_state($table->column('same', Swoole\Table::TYPE_INT), 'first duplicate declaration');
require_state($table->column('same', Swoole\Table::TYPE_STRING, 5), 'second duplicate declaration');
$binary_name = "bin\0name";
require_state($table->column($binary_name, Swoole\Table::TYPE_STRING, 5), 'binary column declaration');
require_state($table->create(), 'duplicate table create');
require_state($table->set('a', ['same' => 7, $binary_name => "B\0N"]), 'set duplicate/binary fields');
require_state($table->get('a', 'same') === 7, 'first-declaration field lookup');
require_state($table->get('a', $binary_name) === "B\0N", 'binary column lookup');
$result['duplicate_whole_row'] = $table->get('a');
$result['duplicate_field'] = $table->get('a', 'same');
$result['duplicate_memory_size'] = $table->getMemorySize();
require_state($table->incr('a', 'same', 2) === 9, 'increment first duplicate field');
require_state($table->set("key\0b", ['same' => 3]), 'binary key');
$iteration = [];
foreach ($table as $key => $row) {
    $iteration[$key] = [$row, $table->get($key, 'same')];
}
ksort($iteration);
require_state(count($iteration) === 2, 'iteration row count');
$result['iteration'] = $iteration;
require_state($table->del('a'), 'delete before increment');
require_state($table->incr('a', 'same', 1) === 1, 'increment after deletion clears fields');
$result['after_delete_increment'] = $table->get('a', 'same');
require_state($table->count() === 2, 'final count');
require_state($table->destroy(), 'duplicate table destroy');
// Recreate the same row after deletion: numeric values must become positive
// zero, and stale string payload must remain invisible behind a cleared length.
$table = new Swoole\Table(64, 0.2);
require_state($table->column('odd', Swoole\Table::TYPE_STRING, 5), 'odd string column');
require_state($table->column('integer', Swoole\Table::TYPE_INT), 'unaligned integer column');
require_state($table->column('double', Swoole\Table::TYPE_FLOAT), 'unaligned double column');
require_state($table->create(), 'typed-clear table create');
for ($i = 0; $i < 128; $i++) {
    require_state($table->set('recycled', ['odd' => "X\0old", 'integer' => PHP_INT_MIN, 'double' => -0.0]), 'seed recycled values');
    require_state($table->del('recycled'), 'delete seeded row');
    if (($i & 1) === 0) {
        require_state($table->set('recycled', []), 'empty set clears recycled fields');
        $expected_integer = 0;
    } else {
        require_state($table->incr('recycled', 'integer', 1) === 1, 'increment initializes recycled fields');
        $expected_integer = 1;
    }
    $row = $table->get('recycled');
    require_state($row['odd'] === '', 'recycled string length cleared');
    require_state($row['integer'] === $expected_integer, 'recycled integer cleared');
    require_state(is_float($row['double']) && $row['double'] === 0.0 && fdiv(1.0, $row['double']) === INF, 'recycled float is positive zero');
}
$result['typed_clear'] = ['recreations' => 128, 'final_row' => $row, 'positive_float_zero' => true];
require_state($table->destroy(), 'typed-clear table destroy');
echo json_encode($result, JSON_THROW_ON_ERROR), "\n";
