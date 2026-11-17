<?php
// Eight processes repeatedly lock one shared row, like Appwrite's domain lookup.
// Keep this separate from the single-worker table set/get instruction benchmark.
$table = new Swoole\Table(64);
$table->column('value', Swoole\Table::TYPE_INT);
$table->create();
$key = md5('benchmark.example');
$table->set($key, ['value' => 1]);

$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_PROCESS);
$server->set(['worker_num' => (int) (getenv('BENCH_TABLE_WORKERS') ?: 8),
    'reactor_num' => 1, 'enable_coroutine' => false]);
$server->on('request', static function ($request, $response) use ($table, $key): void {
    $found = 0;
    for ($i = 0; $i < 1024; $i++) {
        $found += (int) $table->exists($key);
    }
    $response->end("$found\n");
});
$server->start();
