<?php
$mode=$argv[1];
function check(bool $value,string $message):void { if(!$value)throw new RuntimeException($message); }
check(in_array($mode,['a','aaaa','bad-id'],true),'valid mode');
$warnings=[];
set_error_handler(static function(int $severity,string $message)use(&$warnings):bool{$warnings[]=[$severity,$message];return true;});
$process=proc_open([PHP_BINARY,'-n',__DIR__.'/php_dns_peer.php',$mode],[0=>['pipe','r'],1=>['pipe','w'],2=>['pipe','w']],$pipes);
check(is_resource($process),'spawn peer');fclose($pipes[0]);
try {
    stream_set_timeout($pipes[1],15);$address=trim((string)fgets($pipes[1]));
    check(preg_match('/^127\.0\.0\.1:[0-9]+$/D',$address)===1,'peer ready');
    Swoole\Coroutine::set(['dns_server'=>$address,'dns_lookup_random'=>false]);
    $result=null;$error=null;
    Swoole\Coroutine\run(static function()use($mode,&$result,&$error):void{
        $result=Swoole\Coroutine\System::dnsLookup('probe.codec.test',3,$mode==='aaaa'?AF_INET6:AF_INET);
        $error=swoole_last_error();
    });
    check($result===($mode==='bad-id'?false:($mode==='aaaa'?'2001:0db8:0000:0000:0000:0000:0000:0042':'192.0.2.42')),'exact DNS result:'.json_encode([$result,$error]));
    if($mode==='bad-id')check($error===SWOOLE_ERROR_DNSLOOKUP_RESOLVE_FAILED,'exact ID mismatch error');
    $output=stream_get_contents($pipes[1]);$stderr=stream_get_contents($pipes[2]);
    fclose($pipes[1]);fclose($pipes[2]);$exit=proc_close($process);$process=null;
    check($exit===0&&$stderr==='','peer clean exit:'.$stderr);
    $peer=json_decode(trim($output),true,512,JSON_THROW_ON_ERROR);
    check($warnings===[],'no warnings:'.json_encode($warnings));
    echo json_encode(['mode'=>$mode,'result'=>$result,'error'=>$error,'peer'=>$peer,'warnings'=>$warnings],JSON_THROW_ON_ERROR),"\n";
} finally {
    if(is_resource($process)){proc_terminate($process);foreach($pipes as $pipe){if(is_resource($pipe))fclose($pipe);}proc_close($process);}
}
