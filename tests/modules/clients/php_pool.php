<?php
$mode=$argv[1];
function check(bool $condition,string $message):void { if(!$condition)throw new RuntimeException($message); }
check(in_array($mode,['reuse','stale'],true),'valid mode');
$warnings=[];set_error_handler(static function(int $severity,string $message)use(&$warnings):bool{$warnings[]=[$severity,$message];return true;});
$process=proc_open([PHP_BINARY,'-n',__DIR__.'/php_pool_peer.php',$mode],[0=>['pipe','r'],1=>['pipe','w'],2=>['pipe','w']],$pipes);check(is_resource($process),'spawn');fclose($pipes[0]);
try {
    stream_set_timeout($pipes[1],15);$address=trim((string)fgets($pipes[1]));check(preg_match('/^127\.0\.0\.1:([0-9]+)$/D',$address,$matches)===1,'peer ready');
    $port=(int)$matches[1];$id="pool\0witness";
    $first=new Swoole\Client(SWOOLE_SOCK_TCP|SWOOLE_KEEP,false,$id);$weakFirst=WeakReference::create($first);
    check($first->connect('127.0.0.1',$port,3),'first connect');
    check($first->reuse===false&&$first->reuseCount===0,'first fresh');
    check($first->send("one\n")===4,'send one');check($first->recv(4,Swoole\Client::MSG_WAITALL)==="ONE\n",'receive one');
    if($mode==='stale')check(trim((string)fgets($pipes[1]))==='stale-ready','peer FIN sent before pool release');
    check($first->close(),'return to pool');$first=null;gc_collect_cycles();check($weakFirst->get()===null,'first PHP owner collected');
    $second=new Swoole\Client(SWOOLE_SOCK_TCP|SWOOLE_KEEP,false,$id);$weakSecond=WeakReference::create($second);
    check($second->connect('127.0.0.1',$port,3),'second connect');
    check($second->reuse===($mode==='reuse'),'reuse property');check($second->reuseCount===($mode==='reuse'?1:0),'reuse count');
    check($second->send("two\n")===4,'send two');check($second->recv(4,Swoole\Client::MSG_WAITALL)==="TWO\n",'receive two');
    $reuse=$second->reuse;$reuseCount=$second->reuseCount;
    check($second->close(true),'force close/free');$second=null;gc_collect_cycles();check($weakSecond->get()===null,'second PHP owner collected');
    $output=stream_get_contents($pipes[1]);$stderr=stream_get_contents($pipes[2]);fclose($pipes[1]);fclose($pipes[2]);$exit=proc_close($process);$process=null;
    check($exit===0&&$stderr==='','peer clean exit:'.$stderr);$peer=json_decode(trim($output),true,512,JSON_THROW_ON_ERROR);
    check($peer['accepts']===($mode==='reuse'?1:2),'actual connection count');check($peer['eof']===true,'peer EOF');check($warnings===[],'no warnings:'.json_encode($warnings));
    echo json_encode(['mode'=>$mode,'reuse'=>$reuse,'reuse_count'=>$reuseCount,'owners_collected'=>true,'peer'=>$peer,'warnings'=>$warnings],JSON_THROW_ON_ERROR),"\n";
}finally{if(is_resource($process)){proc_terminate($process);foreach($pipes as $pipe){if(is_resource($pipe))fclose($pipe);}proc_close($process);}}
