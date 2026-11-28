<?php
$mode=$argv[1];function check(bool $ok,string $message):void{if(!$ok)throw new RuntimeException($message);}
class ReferenceProbe{private $trace;private string $name;public function __construct(string $name,array &$trace){$this->name=$name;$this->trace=&$trace;}public function __destruct(){$this->trace[]='destroy-'.$this->name;}}
$warnings=[];set_error_handler(static function(int $severity,string $message)use(&$warnings):bool{$warnings[]=[$severity,$message];return true;});$process=null;$summary=[];
try {
 if($mode==='setup'){
  Swoole\Coroutine\run(static function()use(&$summary):void{
   $client=new Swoole\Coroutine\Client(SWOOLE_SOCK_UDP);$weak=WeakReference::create($client);$address='unchanged';$port=-9;
   check($client->recvfrom(0,$address,$port)===false,'nonpositive length');check($address==='unchanged'&&$port===-9,'references untouched');check($client->sendto('127.0.0.1',123,'')===false,'empty datagram');check($client->socket===null&&$client->fd===-1,'no lazy socket for rejected input');
   check($client->isConnected()===false,'not connected');foreach(['peek','getsockname','getpeername','close'] as $method)check($client->$method()===false&&$client->errCode===SWOOLE_ERROR_CLIENT_NO_CONNECTION,'no socket '.$method);
   check($client->socket===null&&$client->connected===false,'no socket published');$client=null;gc_collect_cycles();check($weak->get()===null,'owner GC');$summary=['mode'=>'setup','references_untouched'=>true,'no_socket_published'=>true,'owner_collected'=>true];
  });
 }else {
  $process=proc_open([PHP_BINARY,'-n',__DIR__.'/php_coro_io_peer.php',$mode],[0=>['pipe','r'],1=>['pipe','w'],2=>['pipe','w']],$pipes);check(is_resource($process),'spawn');fclose($pipes[0]);stream_set_timeout($pipes[1],15);$address=trim((string)fgets($pipes[1]));check(preg_match('/^127\.0\.0\.1:([0-9]+)$/D',$address,$matches)===1,'peer ready');$peerPort=(int)$matches[1];$localPort=0;
  Swoole\Coroutine\run(static function()use($mode,$peerPort,&$localPort,&$summary,$pipes):void{
   $client=new Swoole\Coroutine\Client(str_starts_with($mode,'udp')?SWOOLE_SOCK_UDP:SWOOLE_SOCK_TCP);$weak=WeakReference::create($client);check($client->set(['timeout'=>3]),'settings');$trace=[];
   if(str_starts_with($mode,'udp')){
    check($client->sendto('127.0.0.1',$peerPort,"udp\0request")===true,'datagram send/lazy socket');$socketWeak=WeakReference::create($client->socket);$local=$client->getsockname();check(is_array($local)&&$local['host']===$local['address']&&filter_var($local['address'],FILTER_VALIDATE_IP,FILTER_FLAG_IPV4)!==false&&$local['port']>0,'local UDP metadata');$localPort=$local['port'];
    check($client->getpeername()===false&&$client->errCode===SOCKET_ENOTCONN,'unconnected UDP peer query');$savedError=$client->errCode;
    if($mode==='udp-alias'){$alias=new ReferenceProbe('alias',$trace);$data=$client->recvfrom(64,$alias,$alias);check($alias===$peerPort&&$trace===['destroy-alias'],'aliased output and old capture lifetime');$references=['alias'=>'peer-port'];}
    else {$host=new ReferenceProbe('address',$trace);$port=new ReferenceProbe('port',$trace);$data=$client->recvfrom(64,$host,$port);check($host==='127.0.0.1'&&$port===$peerPort&&$trace===['destroy-address','destroy-port'],'reference outputs/destructor order');$references=['address'=>$host,'port'=>'peer-port'];}
    check($data==="udp\0reply"&&$client->errCode===$savedError,'binary datagram/stale public error preserved');$local['port']='local-port';$summary=['mode'=>$mode,'response_hex'=>bin2hex($data),'references'=>$references,'destructors'=>$trace,'local'=>$local,'public_error_preserved'=>true];
   }else {
    check($client->isConnected()===false,'inactive initially');check($client->connect('127.0.0.1',$peerPort,3)&&$client->isConnected()===true,'active connect');$socketWeak=WeakReference::create($client->socket);$local=$client->getsockname();$peer=$client->getpeername();check($local['host']===$local['address']&&$local['address']==='127.0.0.1'&&$local['port']>0,'local TCP metadata');check($peer===['host'=>'127.0.0.1','address'=>'127.0.0.1','port'=>$peerPort],'peer TCP metadata');$localPort=$local['port'];$local['host']='edited';check($local['address']==='127.0.0.1','metadata string copy semantics');
    check(trim((string)fgets($pipes[1]))==='payload-ready','peer payload synchronized');$peek=$client->peek(7);check(is_string($peek)&&$peek!==''&&str_starts_with("PEEK\0OK",$peek),'peek initialized prefix');$data='';while(strlen($data)<7){$part=$client->recv();check(is_string($part)&&$part!=='','receive after peek');$data.=$part;}check($data==="PEEK\0OK",'peek did not consume');check($client->send('ack!')===4,'ack');check(trim((string)fgets($pipes[1]))==='peer-closed','FIN synchronized');check($client->peek(7)==='','peek EOF');$summary=['mode'=>'peek','response_hex'=>bin2hex($data),'peek_hex'=>bin2hex($peek),'peek_eof'=>true,'metadata_copies'=>true];
   }
   check($client->close()===true&&$client->socket===null&&$client->connected===false&&$client->isConnected()===false,'close detaches');check($client->close()===false&&$client->errCode===SWOOLE_ERROR_CLIENT_NO_CONNECTION,'second close');$client=null;gc_collect_cycles();check($weak->get()===null&&$socketWeak->get()===null,'owner/socket GC');$summary+=['owner_collected'=>true,'socket_collected'=>true,'socket_detached'=>true];
  });
  $output=stream_get_contents($pipes[1]);$stderr=stream_get_contents($pipes[2]);fclose($pipes[1]);fclose($pipes[2]);$exit=proc_close($process);$process=null;check($exit===0&&$stderr==='','peer clean:'.$stderr);$peer=json_decode(trim($output),true,512,JSON_THROW_ON_ERROR);if(isset($peer['client'])){check($peer['client']==='127.0.0.1:'.$localPort,'peer observed exact local port');$peer['client']='127.0.0.1:local-port';}$summary['peer']=$peer;
 }
 check($warnings===[],'unexpected warnings:'.json_encode($warnings));$summary['warnings']=$warnings;echo json_encode($summary,JSON_THROW_ON_ERROR),"\n";
}finally{if(is_resource($process)){proc_terminate($process);foreach($pipes as $pipe)if(is_resource($pipe))fclose($pipe);proc_close($process);}}
