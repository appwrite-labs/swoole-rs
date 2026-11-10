<?php
// A Rust-raised warning reaches a user error handler that exit()s or raises a
// fatal error (a bailout). Output and exit code must match upstream's.
// Usage: php error_handler_bailout.php <case> <exit|fatal>
[$case, $mode] = [$argv[1], $argv[2]];

set_error_handler(function (int $level, string $message) use ($mode) {
    echo "handler: $message\n";
    if ($mode === 'exit') {
        exit(7);
    }
    trigger_error('fatal from the handler', E_USER_ERROR);
});

$cases = [
    // Call's ZPP coercion: passing null to an int is deprecated.
    'atomic_coercion' => fn() => (new Swoole\Atomic())->add(null),
    // phpprocess: php_error through crates/zend.
    'process_signal' => fn() => Swoole\Process::signal(9999, fn() => null),
    // phpcollections: a C++ php_swoole_fatal_error reporter.
    'table_column' => fn() => (new Swoole\Table(8))->column('a', Swoole\Table::TYPE_STRING, 0),
    // zval_get_long of an object warns.
    'table_conversion' => function () {
        $table = new Swoole\Table(8);
        $table->column('a', Swoole\Table::TYPE_INT);
        $table->create();
        return $table->set('k', ['a' => new stdClass()]);
    },
    // phptimer: a C++ reporter.
    'timer_after' => fn() => Swoole\Timer::after(0, fn() => null),
    // phpevents: a C++ reporter.
    'event_del' => fn() => Swoole\Event::del(STDIN),
    // phphttp: Cookie's warning.
    'cookie_name' => fn() => (new Swoole\Http\Cookie())->withName('')->toString(),
    // phpclients: php_swoole_sys_error.
    'http_client_file' => fn() => (new Swoole\Coroutine\Http\Client('127.0.0.1', 80))
        ->addFile('/nonexistent/file', 'file'),
    // phphttp: PHPCoroutine::create's warning while Coroutine\Http\Server
    // accepts (#913).
    'server_coro_accept' => function () {
        Swoole\Coroutine::set(['max_coroutine' => 2, 'hook_flags' => 0]);
        return Swoole\Coroutine\run(function () {
        $server = new Swoole\Coroutine\Http\Server('127.0.0.1', 0);
        $server->handle('/', fn() => null);
        go(function () use ($server) {
            $client = new Swoole\Coroutine\Client(SWOOLE_SOCK_TCP);
            $client->connect('127.0.0.1', $server->port);
            $client->recv(0.5);
            // exit() throws in a coroutine and the loop keeps accepting.
            $server->shutdown();
        });
        $server->start();
        });
    },
];

var_dump($cases[$case]());
echo "after\n";
