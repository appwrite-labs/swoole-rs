<?php
// A prepared SELECT per request through hooked PDO mysql (#2066), on one connection per
// worker. MySQL is MYSQL_HOST (default 127.0.0.1), root/root, database test.
// The first request connects: with hooks on, a connect in workerStart yields, and the
// worker serves requests before it returns (#2235).
require __DIR__ . '/../../corpus/bootstrap.php';

$server = httpServer();
$server->set(['hook_flags' => SWOOLE_HOOK_ALL]);
$pdo = null;
$server->on('request', static function ($request, $response) use (&$pdo): void {
    if ($pdo === null) {
        $connection = new PDO('mysql:host=' . (getenv('MYSQL_HOST') ?: '127.0.0.1') . ';dbname=test', 'root', 'root', [
            PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
            PDO::ATTR_EMULATE_PREPARES => false,
        ]);
        $connection->exec('CREATE TABLE IF NOT EXISTS bench_users (id INT PRIMARY KEY, name VARCHAR(32) NOT NULL)');
        $connection->exec("REPLACE INTO bench_users VALUES (42, 'benchmark')");
        $pdo = $connection;
    }
    $statement = $pdo->prepare('SELECT id, name FROM bench_users WHERE id = ?');
    $statement->execute([(int) ($request->get['id'] ?? 0)]);
    jsonResponse($response, $statement->fetch(PDO::FETCH_ASSOC) ?: null);
});
$server->start();
