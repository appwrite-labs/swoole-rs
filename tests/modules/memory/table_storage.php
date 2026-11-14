<?php
// Supplemental public PHP behavior; physical pointer alignment is checked by
// the native odd-width witness, not inferred from this observable transcript.
function storage_require(bool $condition, string $message): void
{
    if (!$condition) {
        throw new RuntimeException($message);
    }
}
storage_require(PHP_INT_SIZE === 8, '64-bit storage witness required');
$memory = [];
for ($width = 1; $width <= 17; ++$width) {
    $table = new Swoole\Table(64, 1.0);
    storage_require($table->getMemorySize() === 0, 'pre-create logical memory');
    storage_require($table->column('integer', Swoole\Table::TYPE_INT), 'integer column');
    storage_require($table->column('binary', Swoole\Table::TYPE_STRING, $width), 'binary column');
    storage_require($table->create(), 'create');
    // PHP rounds requested string capacity to8 before the4-byte length prefix.
    $rounded = ($width + 7) & ~7;
    $legacy_pool = 48 + 64 * (88 + 8 + $rounded + 4 + 24);
    storage_require($table->getMemorySize() === $legacy_pool, 'legacy reported pool bytes');
    storage_require($table->memorySize === $legacy_pool, 'memorySize property');
    $memory[] = [$width, $legacy_pool];
    $expected = [];
    for ($id = 0; $id < 32; ++$id) {
        $key = "odd\0key" . $id;
        $payload = substr(str_repeat("\0v\xff", $width), 0, $width);
        $expected[$key] = ['integer' => $id, 'binary' => $payload];
        storage_require($table->set($key, $expected[$key]), 'set');
        storage_require($table->get($key) === $expected[$key], 'typed roundtrip');
    }
    $seen = [];
    foreach ($table as $key => $row) {
        $seen[$key] = $row;
    }
    ksort($expected);
    ksort($seen);
    storage_require($seen === $expected, 'iteration');
    $key = "odd\0key0";
    $child = new Swoole\Process(static function () use ($table, $key): void {
        for ($i = 0; $i < 64; ++$i) {
            storage_require(is_int($table->incr($key, 'integer')), 'child increment');
        }
    }, false, SOCK_STREAM);
    $pid = $child->start();
    storage_require(is_int($pid) && $pid > 0, 'fork start');
    for ($i = 0; $i < 64; ++$i) {
        storage_require(is_int($table->incr($key, 'integer')), 'parent increment');
    }
    $status = Swoole\Process::wait();
    storage_require(is_array($status) && $status['pid'] === $pid && $status['code'] === 0
        && $status['signal'] === 0, 'child exit');
    storage_require($table->get($key, 'integer') === 128, 'shared increments');
    foreach ($expected as $key => $row) {
        storage_require($table->del($key), 'delete');
        storage_require($table->set($key, []), 'recreate cleared row');
        storage_require($table->get($key) === ['integer' => 0, 'binary' => ''], 'typed clear');
        storage_require($table->del($key), 'delete recreated row');
    }
    storage_require($table->count() === 0, 'empty accounting');
    storage_require($table->getMemorySize() === $legacy_pool, 'stable logical memory');
    storage_require($table->destroy(), 'destroy');
    unset($child, $table);
}
echo json_encode(['schemas' => 17, 'records' => 544, 'recreations' => 544,
    'forks' => 17, 'shared_increments' => 2176, 'legacy_pool_bytes' => $memory], JSON_THROW_ON_ERROR), PHP_EOL;
