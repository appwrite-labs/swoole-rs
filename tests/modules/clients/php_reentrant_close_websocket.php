<?php
// An upgraded client's recv() from a second coroutine while the first waits on a
// frame: the second stops the script with the bound-socket error, as upstream does.
Swoole\Coroutine\run(static function():void{
    $server=new Swoole\Coroutine\Http\Server('127.0.0.1',0);
    $server->handle('/',static function($request,$response):void{
        $response->upgrade();Swoole\Coroutine::sleep(0.05);
        $response->push('one');$response->recv(5);
    });
    Swoole\Coroutine::create(static fn()=>$server->start());
    $client=new Swoole\Coroutine\Http\Client('127.0.0.1',$server->port);
    if(!$client->upgrade('/'))throw new RuntimeException('upgrade');
    Swoole\Coroutine::create(static fn()=>$client->recv(2));
    Swoole\Coroutine::sleep(0.01);
    echo "second recv\n";
    var_dump($client->recv(1));
    echo "unreachable\n";
});
