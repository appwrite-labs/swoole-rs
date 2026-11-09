<?php
// Runtime primitives per request (#1909): Swoole\Table set/get, a Coroutine\Channel
// between two coroutines, and the same server answering HTTP/2 with prior knowledge.
require __DIR__ . '/../../corpus/bootstrap.php';

const ROWS = 64;

$table = new Swoole\Table(ROWS * 2);
$table->column('hits', Swoole\Table::TYPE_INT);
$table->column('name', Swoole\Table::TYPE_STRING, 16);
$table->create();

$server = httpServer();
$server->set(['open_http2_protocol' => true]);
$server->on('request', static function ($request, $response) use ($table): void {
    $sum = 0;
    switch ($request->server['request_uri']) {
        case '/table':
            for ($i = 0; $i < ROWS; $i++) {
                $table->set("row$i", ['hits' => $i, 'name' => "n$i"]);
            }
            for ($i = 0; $i < ROWS; $i++) {
                $sum += $table->get("row$i", 'hits');
            }
            break;
        case '/channel':
            $channel = new Swoole\Coroutine\Channel(1);
            go(static function () use ($channel): void {
                for ($i = 0; $i < ROWS; $i++) {
                    $channel->push($i);
                }
                $channel->close();
            });
            while (($value = $channel->pop()) !== false) {
                $sum += $value;
            }
            break;
        default:
            $response->end("ok\n");
            return;
    }
    $response->end("$sum\n");
});
$server->start();
