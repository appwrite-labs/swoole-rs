<?php
// A persistent Client closed from its own package_length_func while recv() holds
// the connection: the connection is closed, not pooled, and the key reconnects fresh.
function check(bool $condition,string $message):void { if(!$condition)throw new RuntimeException($message); }
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
    echo json_encode(['outer'=>$outer,'fresh'=>$fresh,'peer'=>$peer,'warnings'=>$warnings],JSON_THROW_ON_ERROR),"\n";
}finally{if(is_resource($process)){proc_terminate($process);foreach($pipes as $pipe){if(is_resource($pipe))fclose($pipe);}proc_close($process);}}
