#!/usr/bin/env python3
"""Actual Address numeric parse/assign/loopback bodies against platform inet_pton.

Native sockaddr layouts and platform libc are real; resolver is a deterministic effect adapter, not DNS/runtime proof.
Rust now fills the sockaddr union and calls inet_pton for other families; the resolver stays native behind a function pointer.
"""
import argparse,json
from pathlib import Path
import gate
ROOT=Path(__file__).resolve().parents[3]
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--sanitize',action='store_true');p.add_argument('--rust-sanitized',action='store_true');a=p.parse_args()
 if a.rust_sanitized and not a.sanitize:p.error('--rust-sanitized requires --sanitize')
 out=a.output.parent/(a.output.stem+'-artifacts');out.mkdir(parents=True,exist_ok=True);gate.LOG=out/'commands';gate.LOG.mkdir(exist_ok=True);archive=a.archive
 original,selected=gate.sources(a.source,archive,'src/network/address.cc');signatures=['bool Address::verify_ip(int family, const std::string &str) {','bool Address::verify_port(const int port, const bool for_connect) {','bool Address::assign(SocketType _type, const std::string &_host, int _port, bool _resolve_name) {','bool Address::is_loopback_addr() const {'];before='';after=''
 for signature in signatures:
  before+=gate.body(original,signature)+'\n';after+=gate.body(selected,signature)+'\n'
 before=gate.body(original,'static bool IN_IS_ADDR_LOOPBACK(const in_addr *a) {')+'\n'+before
 after=gate.body(selected,'static int address_resolve(int family, const char *name, char *address) {')+'\n'+after

 fixture=Path(__file__).with_name('ip_witness.cpp');prelude,rest=fixture.read_text().split('// DECLARATIONS');facade=rest.split('// DRIVER')[0];cpp=out/'actual_ip.cpp'
 driver=r'''int main(){std::vector<std::string>inputs={"","0.0.0.0","127.0.0.1","127.1.2.3","255.255.255.255","01.2.3.4","08.009.000.001","::ffff:01.02.03.004","1.2.3.256","1.2.3","+1.2.3.4","1.2.3.4 ","::","::1","::ffff:192.0.2.1","::192.0.2.1","2001:db8::1","1:2:3:4:5:6:7:8","::%lo0","::%","::1%tail","fe80::1%99999999999999999","1.2.3.4%scope","::1%scope:foo","::1%\xff","resolve-fail","localhost",std::string("127.0.0.1\0tail",14),std::string("::1\0tail",8)};for(const auto&seed:std::vector<std::string>{"127.0.0.1","2001:db8::1","::ffff:192.0.2.1"})for(size_t pos=0;pos<seed.size();++pos)for(unsigned byte=0;byte<256;++byte){auto item=seed;item[pos]=static_cast<char>(byte);inputs.push_back(item);}for(unsigned n=0;n<2048;++n){inputs.push_back(std::to_string(n%256)+"."+std::to_string((n/2)%256)+"."+std::to_string((n/3)%256)+"."+std::to_string((n/5)%256));std::ostringstream ipv6;ipv6<<std::hex<<n<<":"<<(n*31)<<"::"<<(n*59);inputs.push_back(ipv6.str());}for(unsigned zeroes=0;zeroes<64;++zeroes){inputs.push_back(std::string(zeroes,'0')+"1.2.3.4");inputs.push_back("::ffff:"+std::string(zeroes,'0')+"1.2.3.4");}for(size_t length:{103,104,105,106,107,108,109,110})inputs.push_back(std::string(length,'x'));size_t cases=0;for(size_t i=0;i<inputs.size();++i){const auto&host=inputs[i];for(int family:{AF_INET,AF_INET6,AF_UNIX,-1}){auto expected=native::verify(family,host),actual=rust::verify(family,host);if(expected!=actual){std::cerr<<"verify:"<<i<<":"<<family<<"\n"<<expected<<"\n"<<actual<<"\n";return 1;}++cases;}if(i<1024||i>=inputs.size()-8)for(int type:{0,1,2,3,4,5,6,7})for(int port:{-1,0,1,65535,65536})for(bool resolve:{false,true}){auto expected=native::assign(type,host,port,resolve),actual=rust::assign(type,host,port,resolve);if(expected!=actual){std::cerr<<"assign:"<<i<<":"<<type<<":"<<port<<":"<<resolve<<"\n"<<expected<<"\n"<<actual<<"\n";return 1;}++cases;}}std::vector<uint8_t>bytes(16);for(unsigned n=0;n<65536;++n){for(unsigned i=0;i<16;++i)bytes[i]=(n>>(i%2*8))&255;for(int type:{0,1,2,3,4,5,6,7}){if(native::loopback(type,bytes)!=rust::loopback(type,bytes))return 1;++cases;}}for(unsigned i=0;i<16;++i){bytes.assign(16,0);bytes[i]=1;for(int type:{0,1,2,3,4,5,6,7}){if(native::loopback(type,bytes)!=rust::loopback(type,bytes))return 1;++cases;}}std::cout<<cases<<"\n";}'''


 # The selected methods use the canonical Address layout and Rust's scratch.
 # Borrow the published addr_str buffer as the same bounded array the original
 # facade seeds/snapshots; preserve every byte comparison and scenario.
 selected_facade=facade.replace('static thread_local char tmp_address[INET6_ADDRSTRLEN];', 'static const in_addr scratch_seed{}; static thread_local char (&tmp_address)[INET6_ADDRSTRLEN] = *reinterpret_cast<char (*)[INET6_ADDRSTRLEN]>(const_cast<char *>(swr_v1_clients_address_string(AF_INET, &scratch_seed)));')
 cpp.write_text(prelude+facade.replace('BODY_NAMESPACE','native').replace('// ACTUAL_BODY',before)+selected_facade.replace('BODY_NAMESPACE','rust').replace('// ACTUAL_BODY',after)+driver);exe=out/'actual_ip';flags=['-fsanitize=address,undefined','-fno-omit-frame-pointer'] if a.sanitize else []
 gate.run(['c++','-std=c++17','-O1',*flags,'-I',ROOT/'native/clients',cpp,archive,'-lpthread','-ldl','-lm','-o',exe]);cases=int(gate.run([exe]).strip())
 report=dict(passed=True,cases=cases,scope=__doc__,pin=gate.PIN,native_sanitized=a.sanitize,rust_sanitized=a.rust_sanitized);a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
if __name__=='__main__':main()
