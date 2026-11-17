<?php
// Runtime primitives per request (#1909): Swoole\Table set/get, a Coroutine\Channel
// between two coroutines, Swoole\Atomic adds and Swoole\Lock lock/unlock (#2654), and the
// same server answering HTTP/2 with prior knowledge. /coroutine-lock and /sleep wait on the
// timer: a Coroutine\Lock taken over from a coroutine that holds it for 1 ms, and a 2 ms
// Coroutine::sleep. Their cost is latency, not instructions.
require __DIR__ . '/../../corpus/bootstrap.php';

const ROWS = 64;

$table = new Swoole\Table(ROWS * 2);
$table->column('hits', Swoole\Table::TYPE_INT);
$table->column('name', Swoole\Table::TYPE_STRING, 16);
$table->create();
$atomic = new Swoole\Atomic();
$lock = new Swoole\Lock(SWOOLE_MUTEX);

$server = httpServer();
$server->set(['open_http2_protocol' => true]);
$server->on('request', static function ($request, $response) use ($table, $atomic, $lock): void {
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
        case '/atomic':
            $atomic->set(0);
            for ($i = 0; $i < ROWS; $i++) {
                $lock->lock();
                $atomic->add($i);
                $lock->unlock();
            }
            $sum = $atomic->get();
            break;
        case '/coroutine-lock':
            // A contended lock waits by sleeping and retrying, as connection pools' locks do.
            $coroutineLock = new Swoole\Coroutine\Lock();
            go(static function () use ($coroutineLock): void {
                $coroutineLock->lock();
                Swoole\Coroutine::sleep(0.001);
                $coroutineLock->unlock();
            });
            $coroutineLock->lock();
            $coroutineLock->unlock();
            $sum = ROWS;
            break;
        case '/sleep':
            Swoole\Coroutine::sleep(0.002);
            $sum = ROWS;
            break;
        default:
            $response->end("ok\n");
            return;
    }
    $response->end("$sum\n");
});
$server->start();
