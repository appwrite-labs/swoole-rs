<?php
$mode=$argv[1];function check(bool $ok,string $message):void{if(!$ok)throw new RuntimeException($message);}
$warnings=[];set_error_handler(static function(int $severity,string $message)use(&$warnings):bool{$warnings[]=[$severity,$message];return true;});$temp=null;
$process=proc_open([PHP_BINARY,'-n',__DIR__.'/php_http_format_peer.php'],[0=>['pipe','r'],1=>['pipe','w'],2=>['pipe','w']],$pipes);check(is_resource($process),'spawn');fclose($pipes[0]);
try {
 stream_set_timeout($pipes[1],15);$address=trim((string)fgets($pipes[1]));check(preg_match('/^127\.0\.0\.1:([0-9]+)$/D',$address,$matches)===1,'peer ready');$port=(int)$matches[1];$proxy=str_starts_with($mode,'proxy-');$host=$mode==='proxy-colon'?'[::1]':'wire.example';$summary=[];
 if($mode==='multipart-file'){$temp=tempnam(sys_get_temp_dir(),'swr-http-');check(is_string($temp)&&file_put_contents($temp,"0FILE\0DATA9")===11,'file fixture');}
 Swoole\Coroutine\run(static function()use($mode,$proxy,$host,$port,$temp,&$summary):void{
  $client=new Swoole\Coroutine\Http\Client($proxy?'origin.example':'127.0.0.1',$proxy?8087:$port);$weak=WeakReference::create($client);$settings=['timeout'=>3,'keep_alive'=>false,'http_compression'=>false];
  if($proxy)$settings+=['http_proxy_host'=>'127.0.0.1','http_proxy_port'=>$port,'http_proxy_user'=>'user','http_proxy_password'=>'pass'];check($client->set($settings),'settings');check($client->setHeaders(['Host'=>$host,'X-Fixture'=>'rust-migration']),'headers');check($client->setBasicAuth('u','p')===null,'native void basic auth setter');check($client->setCookies(['token'=>'a b','binary'=>"A\0B"]),'cookies');
  if($mode==='multipart-memory'){$client->uploadFiles=[PHP_INT_MAX=>[]];check($client->addData('x','full')===false,'no next upload index');$client->uploadFiles=null;}
  if($mode==='multipart-memory')check($client->addData("P\0AYLOAD",'upload','application/octet-stream','memory.bin'),'memory upload');
  if($mode==='multipart-file')check($client->addFile($temp,'upload','application/octet-stream','file.bin',1,9),'file upload');
  $body=match($mode){'raw'=>"body\0raw",'form'=>['bin'=>"A\0B",'space'=>'a b','zero'=>'0'],'multipart-memory','multipart-file'=>['tag'=>"value\0tail"],default=>null};
  $ok=$proxy?$client->get('/wire'):$client->post('/wire',$body);check($ok===true&&$client->statusCode===200&&$client->body==='OK','request/response');check($client->connected===false,'closed after nonkeepalive request');check($client->close()===false,'socket already removed');$client=null;gc_collect_cycles();check($weak->get()===null,'owner collected');$summary=['mode'=>$mode,'status'=>200,'owner_collected'=>true];
 });
 $output=stream_get_contents($pipes[1]);$stderr=stream_get_contents($pipes[2]);fclose($pipes[1]);fclose($pipes[2]);$exit=proc_close($process);$process=null;check($exit===0&&$stderr==='','peer clean:'.$stderr);$wire=json_decode(trim($output),true,512,JSON_THROW_ON_ERROR);$headers=$wire['headers'];check($headers[0]==='Host: '.$host,'Host first');check(in_array('Authorization: Basic dTpw',$headers,true),'basic auth wire');check(in_array('Cookie: token=a+b; binary=A%00B',$headers,true),'cookie wire');check(in_array('Connection: closed',$headers,true),'native closed token');check(in_array('X-Fixture: rust-migration',$headers,true),'custom header');
 $body=hex2bin($wire['body_hex']);check($wire['content_length']===strlen($body),'exact advertised length');
 if($proxy){check($wire['request']==='GET http://'.$host.($mode==='proxy-host'?':8087':'').'/wire HTTP/1.1','absolute proxy URI');check(in_array('Proxy-Authorization: Basic dXNlcjpwYXNz',$headers,true),'proxy auth');check($body==='','proxy empty body');}
 else {
  check($wire['request']==='POST /wire HTTP/1.1','POST request');
  if($mode==='raw')check($body==="body\0raw",'binary raw body');
  elseif($mode==='form')check($body==='bin=A%00B&space=a+b&zero=0','form body');
  else {
   $boundary=null;foreach($headers as $line)if(str_starts_with($line,'Content-Type: multipart/form-data; boundary='))$boundary=substr($line,44);check(is_string($boundary)&&strlen($boundary)===39&&str_starts_with($boundary,'----SwooleBoundary'),'native boundary');
   $payload=$mode==='multipart-file'?"FILE\0DATA":"P\0AYLOAD";$filename=$mode==='multipart-file'?'file.bin':'memory.bin';
   $expected='--'.$boundary."\r\nContent-Disposition: form-data; name=\"tag\"\r\n\r\nvalue\0tail\r\n--".$boundary."\r\nContent-Disposition: form-data; name=\"upload\"; filename=\"".$filename."\"\r\nContent-Type: application/octet-stream\r\n\r\n".$payload."\r\n--".$boundary."--\r\n";check($body===$expected,'exact multipart bytes');
   $wire['headers']=array_map(static fn($line)=>str_replace($boundary,'<BOUNDARY>',$line),$headers);$wire['body_hex']=bin2hex(str_replace($boundary,'<BOUNDARY>',$body));$summary['boundary_validated_and_normalized']=true;
  }
 }
 check($warnings===[],'warnings:'.json_encode($warnings));$summary['wire']=$wire;$summary['warnings']=$warnings;echo json_encode($summary,JSON_THROW_ON_ERROR),"\n";
}finally{if(is_resource($process)){proc_terminate($process);foreach($pipes as $pipe)if(is_resource($pipe))fclose($pipe);proc_close($process);}if(is_string($temp)&&is_file($temp))unlink($temp);}
