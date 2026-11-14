<?php
$mode=$argv[1];function check(bool $ok,string $message):void{if(!$ok)throw new RuntimeException($message);}
$warnings=[];set_error_handler(static function(int $severity,string $message)use(&$warnings):bool{$warnings[]=[$severity,$message];return true;});$process=null;$temp=null;$summary=[];
try {
 if($mode==='setup-errors'){
  Swoole\Coroutine\run(static function()use(&$summary):void{
   $client=new Swoole\Coroutine\Client(SWOOLE_SOCK_TCP);$weak=WeakReference::create($client);check($client->connect('',0)===false,'empty host');check($client->connect('127.0.0.1',0)===false,'invalid TCP port');check($client->send('')===false,'empty send');check($client->sendfile('')===false,'empty sendfile');check($client->send('x')===false&&$client->errCode===SWOOLE_ERROR_CLIENT_NO_CONNECTION,'no-connection send');check($client->fd===-1&&$client->socket===null&&$client->connected===false,'no socket published');$client=null;gc_collect_cycles();check($weak->get()===null,'owner GC');$summary=['setup_errors_matched'=>true,'no_socket_published'=>true,'owner_collected'=>true];
  });
  $expected=['The host is empty','The port is invalid','data to send is empty','file to send is empty'];check(count($warnings)===count($expected),'warning count');foreach($expected as $i=>$text)check($warnings[$i][0]===E_WARNING&&str_ends_with($warnings[$i][1],$text),'warning order');
 }else {
  $process=proc_open([PHP_BINARY,'-n',__DIR__.'/php_coro_peer.php',$mode],[0=>['pipe','r'],1=>['pipe','w'],2=>['pipe','w']],$pipes);check(is_resource($process),'spawn');fclose($pipes[0]);stream_set_timeout($pipes[1],15);$address=trim((string)fgets($pipes[1]));check(preg_match('/^127\.0\.0\.1:([0-9]+)$/D',$address,$matches)===1,'peer ready');$port=(int)$matches[1];
  if($mode==='sendfile'){$temp=tempnam(sys_get_temp_dir(),'swr-coro-');check(is_string($temp)&&file_put_contents($temp,"0F\0ILE9")===7,'file fixture');}
  Swoole\Coroutine\run(static function()use($mode,$port,$temp,&$summary):void{
   $client=new Swoole\Coroutine\Client(SWOOLE_SOCK_TCP);$weak=WeakReference::create($client);$settings=['timeout'=>3];if($mode==='length')$settings+=['open_length_check'=>true,'package_length_type'=>'N','package_length_offset'=>0,'package_body_offset'=>4,'package_max_length'=>8192];if($mode==='eof')$settings+=['open_eof_check'=>true,'package_eof'=>"\r\n",'package_max_length'=>8192];check($client->set($settings),'settings');check($client->connect('127.0.0.1',$port,3),'connect');check($client->connected===true,'connected property');
   if($mode==='duplicate')check($client->connect('127.0.0.1',$port,3)===false&&$client->errCode===SOCKET_EISCONN&&$client->connected===true,'duplicate connect preserves socket');
   $received=[];
   if($mode==='timeout'){check($client->recv(0.02)===false&&$client->errCode===SOCKET_ETIMEDOUT,'timeout error properties');}
   else {
    if($mode==='sendfile')check($client->sendfile($temp,1,5)===true,'sendfile offset');else check($client->send("raw\0data\n")===9,'binary send');
    $expected=match($mode){'length'=>[pack('N',5)."A\0BCD",pack('N',3)."M\0N"],'eof'=>["A\0B\r\n"],'sendfile'=>['OK'],default=>["REPLY\0OK"]};foreach($expected as $value){$data=$client->recv();check($data===$value,'exact receive');$received[]=bin2hex($data);}check($client->recv()==='','EOF result');
   }
   check($client->close()===true&&$client->connected===false&&$client->socket===null,'close/detach');$client=null;gc_collect_cycles();check($weak->get()===null,'owner GC');$summary=['mode'=>$mode,'received_hex'=>$received,'duplicate_checked'=>$mode==='duplicate','timeout_checked'=>$mode==='timeout','owner_collected'=>true,'socket_detached'=>true];
  });
  $output=stream_get_contents($pipes[1]);$stderr=stream_get_contents($pipes[2]);fclose($pipes[1]);fclose($pipes[2]);$exit=proc_close($process);$process=null;check($exit===0&&$stderr==='','peer clean:'.$stderr);$summary['peer']=json_decode(trim($output),true,512,JSON_THROW_ON_ERROR);check($warnings===[],'unexpected warnings:'.json_encode($warnings));
 }
 $summary['warnings']=$warnings;echo json_encode($summary,JSON_THROW_ON_ERROR),"\n";
}finally{if(is_resource($process)){proc_terminate($process);foreach($pipes as $pipe)if(is_resource($pipe))fclose($pipe);proc_close($process);}if(is_string($temp)&&is_file($temp))unlink($temp);}
