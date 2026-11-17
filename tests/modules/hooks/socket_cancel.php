<?php
// A resolver can park more than once, including in another socket operation.
// Cancel/close must reach that operation's canceler and stop the outer connect.
function check(bool $ok, string $message): void {
    if (!$ok) throw new RuntimeException($message);
}
final class ParkedResolver extends Swoole\NameResolver {
    public function __construct() {}
    public function join(string $name, string $ip, int $port, array $options = []): bool { return true; }
    public function leave(string $name, string $ip, int $port): bool { return true; }
    public function getCluster(string $name): ?Swoole\NameResolver\Cluster { return null; }
    public function lookup(string $name) {
        Swoole\Coroutine\System::sleep(0.001);
        Swoole\Coroutine\System::sleep(30);
        return '127.0.0.1';
    }
}
if (($argv[1] ?? '') === 'reactor') swoole_async_set(['enable_coroutine' => false]);
$resolver = new ParkedResolver();
check(swoole_name_resolver_add($resolver), 'register resolver');
foreach (['resolve', 'recv'] as $operation) {
    foreach (['cancel', 'close'] as $action) {
        Swoole\Coroutine\run(function () use ($action, $operation) {
            $socket = new Swoole\Coroutine\Socket(AF_INET, $operation === 'resolve' ? SOCK_STREAM : SOCK_DGRAM, 0);
            if ($operation === 'recv') check($socket->bind('127.0.0.1', 0), 'bind receiver');
            $finished = false;
            Swoole\Coroutine::create(function () use ($socket, $action, $operation, &$finished) {
                $peer = null;
                $result = $operation === 'resolve' ? $socket->connect('parked.fixture.test', 1) : $socket->recvfrom($peer);
                check($result === false, $operation.' was interrupted');
                check($socket->errCode === SOCKET_ECANCELED || ($action === 'close' && $socket->errCode === SWOOLE_ERROR_CO_SOCKET_CLOSE_WAIT), 'socket cancellation error: '.$socket->errCode);
                $finished = true;
            });
            if (($GLOBALS['argv'][1] ?? '') === 'reactor') {
                Swoole\Timer::after(10, function () use ($socket, $action) { check(Swoole\Coroutine::getCid() === -1, 'reactor callback'); $socket->$action(); });
                Swoole\Coroutine\System::sleep(0.05);
            } else {
                Swoole\Coroutine\System::sleep(0.01);
                $socket->$action();
                Swoole\Coroutine\System::sleep(0.01);
            }
            check($finished, 'operation woke through its current wait');
            $socket->close();
        });
    }
}
check(swoole_name_resolver_remove($resolver), 'remove resolver');
echo "socket cancellation passed\n";
