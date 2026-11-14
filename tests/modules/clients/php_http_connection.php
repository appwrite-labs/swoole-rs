<?php
$mode=$argv[1];function check(bool $ok,string $message):void{if(!$ok)throw new RuntimeException($message);}
$warnings=[];set_error_handler(static function(int $severity,string $message)use(&$warnings):bool{$warnings[]=[$severity,$message];return true;});
$process=proc_open([PHP_BINARY,'-n',__DIR__.'/php_http_connection_peer.php',$mode],[0=>['pipe','r'],1=>['pipe','w'],2=>['pipe','w']],$pipes);check(is_resource($process),'spawn');fclose($pipes[0]);
try {
 stream_set_timeout($pipes[1],15);$address=trim((string)fgets($pipes[1]));check(preg_match('/^127\.0\.0\.1:([0-9]+)$/D',$address,$matches)===1,'peer ready');$port=(int)$matches[1];$summary=[];
 Swoole\Coroutine\run(static function()use($mode,$port,$pipes,&$summary):void{
  $client=new Swoole\Coroutine\Http\Client('127.0.0.1',$port);$weak=WeakReference::create($client);$callbacks=[];$close_result=null;$exception=null;
  $settings=['timeout'=>3,'keep_alive'=>true,'max_retries'=>1];
  if(in_array($mode,['close-callback','throw-callback'],true))$settings['write_func']=static function($owner,$data)use($mode,&$callbacks,&$close_result){$callbacks[]=bin2hex($data);if($mode==='throw-callback')throw new RuntimeException('http-write-stop');$close_result=$owner->close();return strlen($data);};
  if($mode==='defer')$settings['defer']=true;
  check($client->set($settings),'settings');$result=null;
  try{$result=$client->get('/first');}catch(Throwable $error){$exception=$error->getMessage();$error=null;}
  if($mode==='reconnect'){
   check($result===true&&$client->body==='A'&&$client->connected===true,'first keepalive result');check(fgets($pipes[1])==="first-closed\n",'stale FIN synchronized');check($client->get('/second')===true&&$client->body==='B','reconnected response');check($client->connected===true,'fresh connected');check($client->close(),'final close');check($client->connected===false,'final disconnected');
  }elseif($mode==='close-callback'){
   check($close_result===true&&$callbacks===['41'],'deferred close inside body callback');check($result===false&&$client->connected===false,'outer receive fails after deferred close');check($client->statusCode===SWOOLE_HTTP_CLIENT_ESTATUS_SERVER_RESET,'reset status');
  }elseif($mode==='throw-callback'){
   check($exception==='http-write-stop'&&$callbacks===['41'],'write exception propagates');$client->close();
  }elseif($mode==='defer'){
   check($result===true&&$client->connected===true,'deferred send');check($client->close()===true&&$client->connected===false,'close before recv');check($client->recv()===false,'wait-response reset');
  }elseif($mode==='refused'){
   check($result===false&&$client->connected===false,'failed connect');check($client->statusCode===SWOOLE_HTTP_CLIENT_ESTATUS_CONNECT_FAILED&&$client->errCode!==0,'connect failure properties');check($client->close()===false,'failed socket already removed');
  }else throw new RuntimeException('mode');
  $summary=['mode'=>$mode,'get_result'=>$result,'exception'=>$exception,'callbacks'=>$callbacks,'callback_close'=>$close_result,'connected'=>$client->connected,'status'=>$client->statusCode];$client=null;gc_collect_cycles();check($weak->get()===null,'HTTP owner collected');$summary['owner_collected']=true;
 });
 $output=stream_get_contents($pipes[1]);$stderr=stream_get_contents($pipes[2]);fclose($pipes[1]);fclose($pipes[2]);$exit=proc_close($process);$process=null;check($exit===0&&$stderr==='','peer clean:'.$stderr);$summary['peer']=json_decode(trim($output),true,512,JSON_THROW_ON_ERROR);check($warnings===[],'warnings:'.json_encode($warnings));$summary['warnings']=$warnings;echo json_encode($summary,JSON_THROW_ON_ERROR),"\n";
}finally{if(is_resource($process)){proc_terminate($process);foreach($pipes as $pipe)if(is_resource($pipe))fclose($pipe);proc_close($process);}}
