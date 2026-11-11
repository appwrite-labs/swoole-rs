<?php
// A persistent Client closed from its own package_length_func while recv() holds
// the connection: the connection is closed, not pooled, and the key reconnects fresh.
// A Coroutine\Http\Client whose write_func tries get, post and recv (each fails, and
// the response it is in completes), replaces itself and calls its client,
// while another coroutine uses the client across the request's yields.
function check(bool $condition,string $message):void { if(!$condition)throw new RuntimeException($message); }
function http_reentry():array {
    $result=[];
    Swoole\Coroutine\run(static function()use(&$result):void{
        $server=new Swoole\Coroutine\Http\Server('127.0.0.1',0);
        $server->handle('/',static function($request,$response):void{
            foreach(['a','b','c'] as $chunk){$response->write($chunk);Swoole\Coroutine::sleep(0.01);}
            $response->end();
        });
        Swoole\Coroutine::create(static fn()=>$server->start());
        $client=new Swoole\Coroutine\Http\Client('127.0.0.1',$server->port);
        $first=[];$second=[];$nested=[];
        $later=static function($c,string $data)use(&$second):void{$second[]=[$data,$c->getStatusCode(),$c->getDefer()];};
        check($client->set(['timeout'=>5,'write_func'=>static function($c,string $data)use(&$first,&$nested,$later):void{
            $first[]=$data;
            foreach(['get'=>static fn()=>$c->get('/'),'post'=>static fn()=>$c->post('/','x'),'recv'=>static fn()=>$c->recv()] as $name=>$call){
                $nested[$name]=[$call(),$c->errCode===SOCKET_EINPROGRESS];
            }
            check($c->set(['write_func'=>$later,'timeout'=>5]),'replace write_func from itself');
        }]),'set write_func');
        $busy=true;$uses=0;
        Swoole\Coroutine::create(static function()use(&$client,&$busy,&$uses):void{
            while($busy){check($client->set(['timeout'=>5])&&$client->getDefer()===false,'use while yielded');$uses++;Swoole\Coroutine::sleep(0.002);}
        });
        $get=$client->get('/');
        $result['get']=['ok'=>$get,'status'=>$client->statusCode,'first'=>$first,'second'=>$second,'nested'=>$nested];
        $client->close();$client=new Swoole\Coroutine\Http\Client('127.0.0.1',$server->port);
        $file=tempnam(sys_get_temp_dir(),'reentry');
        $download=$client->download('/',$file);
        $busy=false;
        $result['download']=['ok'=>$download,'status'=>$client->statusCode,'body'=>file_get_contents($file)];
        $result['used_while_yielded']=$uses>0;
        unlink($file);$client->close();$server->shutdown();
    });
    return $result;
}
$warnings=[];set_error_handler(static function(int $severity,string $message)use(&$warnings):bool{$warnings[]=[$severity,$message];return true;});
$process=proc_open([PHP_BINARY,'-n',__DIR__.'/php_reentrant_close_peer.php'],[0=>['pipe','r'],1=>['pipe','w'],2=>['pipe','w']],$pipes);check(is_resource($process),'spawn');fclose($pipes[0]);
try {
    stream_set_timeout($pipes[1],15);$address=trim((string)fgets($pipes[1]));check(preg_match('/^127\.0\.0\.1:([0-9]+)$/D',$address,$matches)===1,'peer ready');
    $port=(int)$matches[1];$id='reentrant-close';
    $first=new Swoole\Client(SWOOLE_SOCK_TCP|SWOOLE_KEEP,false,$id);$calls=0;
    $first->set(['open_length_check'=>true,'package_length_func'=>static function(string $data)use(&$first,&$calls):int{
        $calls++;check($first->close(),'reentrant close');return -1;
    }]);
    check($first->connect('127.0.0.1',$port,3),'first connect');check($first->reuse===false,'first fresh');
    check($first->send("one\n")===4,'send one');
    $received=$first->recv();
    check($calls===1,'length function ran once');
    $outer=['received'=>$received,'errCode'=>$first->errCode,'connected'=>$first->isConnected()];
    $second=new Swoole\Client(SWOOLE_SOCK_TCP|SWOOLE_KEEP,false,$id);
    check($second->connect('127.0.0.1',$port,3),'second connect');
    $fresh=['reuse'=>$second->reuse,'reuse_count'=>$second->reuseCount];
    check($second->send("two\n")===4,'send two');check($second->recv(4,Swoole\Client::MSG_WAITALL)==="TWO\n",'receive two');
    check($second->close(true),'force close');
    $output=stream_get_contents($pipes[1]);$stderr=stream_get_contents($pipes[2]);fclose($pipes[1]);fclose($pipes[2]);$exit=proc_close($process);$process=null;
    check($exit===0&&$stderr==='','peer clean exit:'.$stderr);$peer=json_decode(trim($output),true,512,JSON_THROW_ON_ERROR);
    $http=http_reentry();
    echo json_encode(['outer'=>$outer,'fresh'=>$fresh,'peer'=>$peer,'http'=>$http,'warnings'=>$warnings],JSON_THROW_ON_ERROR),"\n";
}finally{if(is_resource($process)){proc_terminate($process);foreach($pipes as $pipe){if(is_resource($pipe))fclose($pipe);}proc_close($process);}}
