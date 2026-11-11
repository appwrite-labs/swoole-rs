#!/usr/bin/env python3
"""enableSSL() on a plain Client (sync and Async) applies its ssl_* settings: verify_peer rejects a self-signed peer."""
import argparse
import json
from pathlib import Path
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report

CASES=dict(verify=False,trusted=True,wrong_host=False)
EXPECTED=dict(sync=CASES,async_=CASES,warnings=[])
# The client logs each rejection: the self-signed certificate without a cafile, the name with one.
LOGS=dict(self_signed=2,host=2)

def serve(listener,context):
    while True:
        try:connection,_=listener.accept()
        except OSError:return
        threading.Thread(target=echo,args=(connection,context),daemon=True).start()

def echo(connection,context):
    try:
        with context.wrap_socket(connection,server_side=True) as tls:
            while data:=tls.recv(4096):tls.sendall(data)
    except (OSError,ssl.SSLError):pass
    finally:connection.close()

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[])
    p.add_argument('--engine',action='append',required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    report=dict(passed=False,scope=__doc__,results=[])
    try:
        with tempfile.TemporaryDirectory() as directory:
            cert,key=Path(directory,'cert.pem'),Path(directory,'key.pem')
            subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1','-subj','/CN=localhost',
                            '-addext','subjectAltName=DNS:localhost','-keyout',key,'-out',cert],check=True,capture_output=True)
            context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(cert,key)
            with socket.create_server(('127.0.0.1',0)) as listener:
                threading.Thread(target=serve,args=(listener,context),daemon=True).start()
                port=listener.getsockname()[1]
                for engine in parse_engines(a.engine,a.php,a.php_extension):
                    result=run_isolated(engine.command(Path(__file__).with_suffix('.php'),port,cert),timeout=35)
                    row=dict(engine=engine.name,exit=result.returncode,stdout=result.stdout,stderr=result.stderr);report['results'].append(row)
                    if result.returncode or result.stderr:raise ValueError(f'{engine.name}: fixture failed; raw outputs preserved')
                    *logs,last=result.stdout.splitlines() or ['']
                    counts=dict(self_signed=sum('self signed certificate' in line for line in logs),
                                host=sum('X509_check_host(): no match' in line for line in logs))
                    if json.loads(last)!=EXPECTED or counts!=LOGS or len(logs)!=sum(LOGS.values()):
                        raise ValueError(f'{engine.name}: unexpected outcome')
        report['passed']=True
    except Exception as error:report['error']=f'{type(error).__name__}: {error}'
    write_report(a.output,report);print(json.dumps({key:report.get(key) for key in ['passed','error']}));return int(not report['passed'])
if __name__=='__main__':sys.exit(main())
