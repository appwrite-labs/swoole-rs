<?php
// One round trip per request through a database or network hook (#2653), on the build with
// --enable-swoole-pgsql, --enable-swoole-sqlite, --enable-swoole-curl and --enable-cares:
// /pgsql and /sqlite run a SELECT prepared once per worker through hooked PDO (the swoole
// build's own pgsql driver, so PHP's pdo_pgsql isn't loaded), /curl a GET to callgrind.py's
// HTTP peer (PEER_HTTP_PORT) with SWOOLE_HOOK_NATIVE_CURL, and /dns a c-ares lookup that
// callgrind.py's resolver (PEER_DNS_PORT) answers. PostgreSQL is PGSQL_HOST (default
// 127.0.0.1), root/root, database test; SQLite is a file under TEST_DATA. Each handle is the
// worker's own, opened by its route's first request: with hooks on, a connect in workerStart
// yields, and the worker serves requests before it returns (#2235).
require __DIR__ . '/../../corpus/bootstrap.php';

// No DNS cache, so every request asks the resolver.
swoole_async_set(['dns_server' => '127.0.0.1:' . getenv('PEER_DNS_PORT'), 'dns_cache_refresh_time' => 0]);

function database(string $dsn, ?string $user = null, ?string $password = null): PDO {
    $pdo = new PDO($dsn, $user, $password, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]);
    $pdo->exec('CREATE TABLE IF NOT EXISTS bench_users (id INT PRIMARY KEY, name VARCHAR(32) NOT NULL)');
    $pdo->exec('DELETE FROM bench_users');
    $pdo->exec("INSERT INTO bench_users VALUES (42, 'benchmark')");
    return $pdo;
}

$server = httpServer();
$server->set(['hook_flags' => SWOOLE_HOOK_ALL]);
$handles = [];
$server->on('request', static function ($request, $response) use (&$handles): void {
    $route = $request->server['request_uri'];
    switch ($route) {
        case '/pgsql':
        case '/sqlite':
            // The statement is the worker's too: on both engines, a hooked pgsql statement freed
            // when its request's coroutine ends leaves the connection busy for the next request
            // ("another command is already in progress").
            $handles[$route] ??= ($route === '/pgsql'
                ? database('pgsql:host=' . (getenv('PGSQL_HOST') ?: '127.0.0.1') . ';dbname=test', 'root', 'root')
                : database('sqlite:' . getenv('TEST_DATA') . '/bench.db'))
                ->prepare('SELECT name FROM bench_users WHERE id = ?');
            $statement = $handles[$route];
            $statement->execute([42]);
            $ok = $statement->fetchColumn() === 'benchmark';
            break;
        case '/curl':
            if (!isset($handles[$route])) {
                $handles[$route] = curl_init('http://127.0.0.1:' . getenv('PEER_HTTP_PORT') . '/');
                curl_setopt($handles[$route], CURLOPT_RETURNTRANSFER, true);
            }
            $ok = curl_exec($handles[$route]) === 'pong';
            break;
        case '/dns':
            $ok = Swoole\Coroutine\System::dnsLookup('callgrind.test') === '127.0.0.1';
            break;
        default:
            $ok = false;
    }
    $response->status($ok ? 200 : 502);
    $response->end($ok ? "ok\n" : "hook failed\n");
});
$server->start();
