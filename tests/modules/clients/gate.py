#!/usr/bin/env python3
"""Actual pinned/patched Address URL/port and hosts-file body differential.

Uses native effect recording doubles, not sockets/PHP. Production TU compilation
and unchanged original/API tests remain separate gates. No network operations.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import apply_patch_verified, module_patches
from tools.build_upstream import COMMIT as PIN



def overlay_manifest():
    return module_patches(ROOT/'migration/patches/clients')[0]


def apply_overlay(scratch, output):
    manifest = overlay_manifest()
    receipt = apply_patch_verified(scratch, ROOT/'migration/patches/clients'/manifest['path'],
                                   manifest['upstream_paths'], manifest['sha256'])
    (output/'overlay-application.json').write_text(json.dumps(receipt, indent=2)+'\n')
    return receipt

LOG = None
SERIAL = 0
# A driver built with SHARDED_MAIN visits every SHARD_COUNT-th outer value from SHARD_INDEX.
SHARDED_MAIN = ('#include <cstdlib>\nstatic unsigned SHARD_INDEX = 0, SHARD_COUNT = 1;\n'
                'static int sharded_main();\nint main(int argc, char **argv) {\n'
                '    if (argc == 3) { SHARD_INDEX = std::strtoul(argv[1], nullptr, 10); SHARD_COUNT = std::strtoul(argv[2], nullptr, 10); }\n'
                '    return sharded_main();\n}\n')


def run_shards(executables, count=None):
    """Run every executable as `count` shards at once; return each one's summed case count."""
    count = count or os.cpu_count() or 1
    jobs = [(exe, subprocess.Popen([str(exe), str(i), str(count)], stdout=subprocess.PIPE, stderr=subprocess.PIPE))
            for exe in executables for i in range(count)]
    totals = {}
    for exe, job in jobs:
        out, err = job.communicate()
        if job.returncode:
            raise subprocess.CalledProcessError(job.returncode, job.args, out, err)
        totals[exe] = totals.get(exe, 0) + int(out.strip())
    return [totals[exe] for exe in executables]

def run(args):
    global SERIAL
    command = list(map(str, args))
    # A program linking the Rust archive needs the system zlib its libz-sys binding
    # (#583) uses, as the extension build already links it.
    if command[0] in ('c++', 'cc') and any(part.endswith('.a') for part in command):
        command += subprocess.check_output(['pkg-config', '--libs', 'zlib'], text=True).split()
    result = subprocess.run(command, capture_output=True)
    record = {"command": command, "returncode": result.returncode}
    for name in ("stdout", "stderr"):
        raw = getattr(result, name)
        try:
            record[name] = raw.decode("utf-8")
        except UnicodeDecodeError:
            record[name] = raw.decode("utf-8", errors="backslashreplace")
            record[name + "_raw_base64"] = base64.b64encode(raw).decode("ascii")
    if LOG is not None:
        SERIAL += 1
        (LOG / f"{SERIAL:03d}.json").write_text(json.dumps(record, indent=2) + "\n")
    result.check_returncode()
    # Keep the existing strict success contract; recording invalid bytes never
    # converts a failed decode/warned tool result into a passing gate.
    result.stderr.decode("utf-8")
    return result.stdout.decode("utf-8")


def body(text, signature):
    start = text.index(signature)
    opening = text.index('{', start)
    depth = 1
    pos = opening + 1
    # These actual bodies contain no brace characters within literals/comments.
    while depth:
        depth += (text[pos] == '{') - (text[pos] == '}')
        pos += 1
    return text[start:pos]

def main():
    p = argparse.ArgumentParser()
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--archive', type=Path)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--sanitize', action='store_true')
    p.add_argument('--rust-sanitized', action='store_true', help='Record independently built archive instrumentation; requires --archive and --sanitize')
    a = p.parse_args()
    report_path = a.output
    a.output = report_path.parent / (report_path.stem + "-artifacts")
    if a.rust_sanitized and not (a.archive and a.sanitize):
        p.error("--rust-sanitized requires --archive and --sanitize")
    a.output.mkdir(parents=True, exist_ok=True)
    global LOG
    LOG = a.output / "commands"
    LOG.mkdir(exist_ok=True)
    archive = a.archive or a.output / 'libclients.a'
    if not a.archive:
        run(['rustc', '--edition', '2024', '--crate-type', 'staticlib', '-O', '-C', 'panic=abort',
             ROOT/'crates/clients/src/lib.rs', '-o', archive])
    scratch = a.output/'patched'
    scratch.mkdir(exist_ok=True)
    run(['git', 'init', '-q', scratch])
    originals = {}
    for path in overlay_manifest()['upstream_paths']:
        name = Path(path).stem
        text = run(['git', '-C', a.source, 'show', f'{PIN}:{path}'])
        if path.startswith('src/network/'):
            originals[name] = text
        dest = scratch/path
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(text)
    apply_overlay(scratch, a.output)
    prefix = r'''
#include <algorithm>
#include <array>
#include <cstdint>
#include <memory>
#include <iostream>
#include <fstream>
#include <unordered_map>
#include <locale>
#include <regex>
#include <sstream>
#include <string>
#include <vector>
#include "swoole_rs_clients.h"
#define SW_PATH_HOSTS "/nonexistent-readiness-hosts"
struct { std::string dns_hosts_path; } SwooleG;
#define SW_ERR -1
#define SW_OK 0
#define SW_LOG_NOTICE 0
#define SW_ERROR_BAD_HOST_ADDR 1
#define SW_TRACE_CLIENT 0
static int logs=0, last_error=0, effect_result=0, calls=0, effect_how=0;
#define swoole_trace_log(...) ((void) 0)
#define swoole_error_log(...) (++logs)
static void swoole_set_last_error(int error) { last_error=error; }
enum SocketType {SW_SOCK_TCP=1, SW_SOCK_UDP, SW_SOCK_TCP6, SW_SOCK_UDP6, SW_SOCK_UNIX_STREAM, SW_SOCK_UNIX_DGRAM};
struct Socket { int fd=123; };
struct Address {
    int type=87; bool accept=true; std::string seen_host; int seen_port=0;
    static bool verify_port(int port, bool for_connect);
    bool assign(const std::string &url);
    bool assign(int kind,const std::string&host,int port) { type=kind;seen_host=host;seen_port=port;return accept; }
};
'''
    funcs = [('address', 'bool Address::verify_port('), ('address', 'bool Address::assign(const std::string &url)'), ('dns', 'std::string get_ip_by_hosts(')]
    extracted = {}
    for engine in ['native', 'rust']:
        texts = originals if engine == 'native' else {n:(scratch/f'src/network/{n}.cc').read_text() for n in originals}
        extracted[engine] = '\n'.join(body(texts[name], sig) for name, sig in funcs)
    for index, (_, signature) in enumerate(funcs):
        native_body = body(extracted['native'], signature)
        selected_body = body(extracted['rust'], signature)
        if native_body == selected_body or 'swr_v1_clients_' not in selected_body:
            raise ValueError(f'Selected body was not converted: {signature}')
    driver = r'''
static std::string trace(const std::string &url, bool accept) {
    Address a;a.accept=accept;logs=0;std::string exception;
    bool ok=false;
    try {ok=a.assign(url);} catch(const std::out_of_range&) {exception="range";} catch(const std::invalid_argument&) {exception="invalid";}
    std::ostringstream s;s<<ok<<":"<<a.type<<":"<<a.seen_host.size()<<":"<<a.seen_host<<":"<<a.seen_port<<":"<<logs<<":"<<exception;
    return s.str();
}
int main(int argc, char **argv) {
    if(argc!=2)return 5;
    SwooleG.dns_hosts_path=argv[1];
    std::vector<std::string> urls={"", "unix:///", "unix:////", "udg:///a", "tcp://a:0", "tcp://a:65536", "tcp://a:999999999999999999999", "tcp://[::1]:80", "tcp://[]:80", "tcp://a:01", "tcp://a:+1", "tcp://a:1\n", "unix:///a?b", "unix:///a#b", "udp://]:80", std::string("tcp://a\0b:80",12)};
    const std::vector<std::string> protocols={"tcp://","udp://","unix://","udg://","TCP://",""};
    const std::vector<std::string> hosts={"a","", "[::1]", "[a]", "[", "]", "a:b", "a[b", "/", "//", "/a", "/a?", "/a#", "a\nb", "a b", std::string("a\0b",3)};
    const std::vector<std::string> ports={"",":",":0",":1",":80",":65535",":65536",":-1",":+1",":0002",":1a",":1:2",":99999999999999999999999"};
    for(const auto&p:protocols)for(const auto&h:hosts)for(const auto&port:ports)urls.push_back(p+h+port);
    for(unsigned i=0;i<256;i++) {urls.push_back("tcp://a:"+std::string(1, char(i)));urls.push_back("tcp://"+std::string(1,char(i))+":80");urls.push_back("unix:///"+std::string(1,char(i)));}
    for(const auto&url:urls)for(bool accept:{false,true}) {auto a=native::trace(url,accept),b=rust::trace(url,accept);if(a!=b) {std::cerr<<"URL mismatch size "<<url.size()<<"\n";return 1;}}
    size_t count=urls.size()*2;
    for(int port=-3;port<=65538;port++)for(bool connect:{false,true}) {if(native::Address::verify_port(port,connect)!=rust::Address::verify_port(port,connect))return 2;++count;}
    struct ExtraWhitespace : std::ctype<char> {
        static const mask *classification() {
            static const std::array<mask, table_size> table=[] {
                std::array<mask,table_size> values{};
                std::copy_n(classic_table(),table_size,values.begin());
                values[0]|=space; values[255]|=space; values['@']|=space;
                return values;
            }();
            return table.data();
        }
        ExtraWhitespace():std::ctype<char>(classification()){}
    };
    for(bool custom:{false,true}) {
    const auto previous=std::locale();
    if(custom)std::locale::global(std::locale(previous,new ExtraWhitespace));
    const std::vector<std::string> hostlines={"", "\n", "127.0.0.1", "127.0.0.1 localhost", "# 127.0.0.1 aaa", "       127.0.0.1 bbb               ccc      #ddd", "a b#c d", "a b#c d#e", "a b b", "a b\nz b", "a b\r\nz b", "\ra b", std::string("a b\0c",5)};
    std::vector<std::string> names={"a","b","localhost","bbb","ccc","ddd","d","d#e","b#c",std::string("b\0c",3),"missing",""};
    for(const auto&line:hostlines) {
        {std::ofstream f(SwooleG.dns_hosts_path, std::ios::binary);f.write(line.data(),line.size());}
        for(const auto&name:names) {if(native::get_ip_by_hosts(name)!=rust::get_ip_by_hosts(name))return 4;++count;}
    }
    for(unsigned i=0;i<256;i++) {
        std::string token(1,char(i));std::string line="addr "+token+" second\nother "+token;
        {std::ofstream f(SwooleG.dns_hosts_path,std::ios::binary);f.write(line.data(),line.size());}
        for(const auto&name:std::vector<std::string>{token,"second","other","missing"}) {if(native::get_ip_by_hosts(name)!=rust::get_ip_by_hosts(name))return 4;++count;}
    }
    std::locale::global(previous);
    }
    std::remove(SwooleG.dns_hosts_path.c_str());
    std::cout<<count<<"\n";
}
'''
    # Same exact native effect types, duplicated method definitions in isolated namespaces.
    # Prefix declarations need namespace-local Address/Client, shared effect recorders.
    split = prefix.index('struct Address')
    source = prefix[:split]+'namespace native {\n'+prefix[split:]+extracted['native']+driver[:driver.index('int main(')]+ '\n}\n'
    source += '#define SW_USE_RUST_CLIENTS 1\nnamespace rust {\n'+prefix[split:]+extracted['rust']+driver[:driver.index('int main(')]+'\n}\n'
    source += driver[driver.index('int main('):]
    cpp=a.output/'actual_callers.cpp';cpp.write_text(source)
    exe=a.output/'actual_callers'
    flags=['-fsanitize=address,undefined','-fno-omit-frame-pointer'] if a.sanitize else []
    compile_cmd=['c++','-std=c++17','-O1',*flags,'-I',ROOT/'native/clients',cpp,archive,'-lpthread','-ldl','-lm','-o',exe]
    run(compile_cmd)
    cases=int(run([exe, a.output/"hosts-fixture"]).strip())
    report={'passed':True,'cases':cases,'scope':'actual pinned/patched address bodies with effect doubles and hosts body with private real file IO; not whole TU/runtime/PHP parity','pin':PIN,'sanitized_native':a.sanitize,'rust_sanitized':a.rust_sanitized,'archive':str(archive.resolve())}
    report_path.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))

if __name__=='__main__':
    main()
