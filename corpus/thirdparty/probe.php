<?php
// External harness: boot the unchanged application's real container/config.
declare(strict_types=1);
use Hyperf\Di\ClassLoader;
use Hyperf\Cache\Driver\SqliteDriver;
use Hyperf\Guzzle\ClientFactory;
use function Hyperf\Coroutine\run;

define('BASE_PATH', getenv('TEST_APP'));
require BASE_PATH . '/vendor/autoload.php';
ClassLoader::init();
$container = require BASE_PATH . '/config/container.php';
$mode = $argv[1];
run(function () use ($container, $mode): void {
    $cache = new SqliteDriver($container, ['database' => getenv('DB_DATABASE'), 'table' => 'corpus', 'prefix' => '']);
    if ($mode === 'write') {
        $client = $container->get(ClientFactory::class)->create(['timeout' => 5]);
        $response = $client->get('http://127.0.0.1:9501/?user=Coroutine%20IO');
        $body = json_decode((string) $response->getBody(), true, flags: JSON_THROW_ON_ERROR);
        if ($response->getStatusCode() !== 200 || $body !== ['method' => 'GET', 'message' => 'Hello Coroutine IO.']) {
            throw new RuntimeException('Framework coroutine HTTP reply differs');
        }
        if (!$cache->set('record', "persisted λ\0bytes")) {
            throw new RuntimeException('Framework cache write failed');
        }
    } elseif ($mode !== 'read') {
        throw new RuntimeException('Unknown probe mode');
    }
    $value = $cache->get('record');
    if ($value !== "persisted λ\0bytes") {
        throw new RuntimeException('Framework persisted value differs');
    }
    $rows = (int) (new PDO('sqlite:' . getenv('DB_DATABASE')))->query('SELECT COUNT(*) FROM corpus')->fetchColumn();
    echo json_encode(['mode' => $mode, 'rows' => $rows,
        'value_hex' => bin2hex($value), 'coroutine' => \Swoole\Coroutine::getCid() > 0], JSON_THROW_ON_ERROR), "\n";
    \Hyperf\Coordinator\CoordinatorManager::until(\Hyperf\Coordinator\Constants::WORKER_EXIT)->resume();
}, 0); // Select Hyperf's coroutine HTTP handler, rather than native curl hooks.
