<?php
// Swoole\Table under contention (#4730): every worker process reads the same row, as an
// app does when it checks one hot key (Appwrite's certified domain) on each request.
require __DIR__ . '/../../corpus/bootstrap.php';

const READS = 64;

$table = new Swoole\Table(64);
$table->column('value', Swoole\Table::TYPE_INT);
$table->create();
$table->set('hot', ['value' => 1]);

$server = httpServer();
$server->set(['worker_num' => swoole_cpu_num() * 6]);
$server->on('request', static function ($request, $response) use ($table): void {
    $hits = 0;
    for ($i = 0; $i < READS; $i++) {
        $hits += (int) $table->exists('hot');
    }
    $response->end("$hits\n");
});
$server->start();
