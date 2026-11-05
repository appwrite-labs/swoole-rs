// SPDX-License-Identifier: Apache-2.0
// Deterministic original-trace fixture independently prepared by M01.

#undef access
#undef mkdir
static std::string unhex(const std::string&s){std::string r;for(size_t i=0;i<s.size();i+=2)r.push_back(char(std::stoul(s.substr(i,2),nullptr,16)));return r;}
static std::string hex(const std::string&s){static const char*d="0123456789abcdef";std::string r;for(unsigned char c:s){r+=d[c>>4];r+=d[c&15];}return r;}
#include <filesystem>
#include <fstream>
static int real_filesystem(const std::string &root) {
    namespace fs = std::filesystem;
    scenario = "real";
    trace_enabled = true;
    auto require = [](bool condition) { if (!condition) std::abort(); };
    const mode_t original_mask = umask(0027);
    require(swoole_mkdir_recursive(root + "/created/deep"));
    require(fs::is_directory(root + "/created/deep"));
    struct stat info = {};
    require(::stat((root + "/created/deep").c_str(), &info) == 0);
    require((info.st_mode & 0777) == 0750);
    require(swoole_mkdir_recursive(root + "//repeated///leaf/"));
    require(fs::is_directory(root + "/repeated/leaf"));
    const fs::path old_cwd = fs::current_path();
    fs::current_path(root);
    require(swoole_mkdir_recursive("relative//leaf"));
    fs::current_path(old_cwd);
    require(fs::is_directory(root + "/relative/leaf"));
    require(swoole_mkdir_recursive(root + "/target/"));
    fs::create_directory_symlink(root + "/target", root + "/link");
    require(swoole_mkdir_recursive(root + "/link/leaf"));
    require(fs::is_directory(root + "/target/leaf"));
    { std::ofstream file(root + "/file"); file << "untouched"; }
    require(swoole_mkdir_recursive(root + "/file"));
    require(!swoole_mkdir_recursive(root + "/file/blocked/deeper"));
    require(errno == ENOTDIR);
    require(fs::is_regular_file(root + "/file"));
    require(swoole_mkdir_recursive(root + "/nul" + std::string("\0/ignored", 9)));
    require(fs::is_directory(root + "/nul"));
    require(!fs::exists(root + "/nul/ignored"));
    require(swoole_mkdir_recursive(root + "/nultail" + std::string("\0/", 2)));
    require(!fs::exists(root + "/nultail"));
    umask(original_mask);
    std::cout << "real filesystem PASS\n";
    return 0;
}
int main(int argc,char**argv){
if(argc==3 && std::string(argv[1])=="--real") return real_filesystem(argv[2]);
std::string line;
while(std::getline(std::cin,line)){std::istringstream in(line);std::string id,p;in>>id>>scenario>>p;
std::string path;
if(p=="@MAX-2")path=std::string(PATH_MAX-2,'a');
else if(p=="@MAX-1-TRAIL")path=std::string(PATH_MAX-2,'a')+"/";
else if(p=="@MAX")path=std::string(PATH_MAX,'a');
else if(p=="@MAX-TRAIL")path=std::string(PATH_MAX-1,'a')+"/";
else if(p=="@MAX+1-NUL")path=std::string("a\0",2)+std::string(PATH_MAX-1,'x');
else if(p=="@MAX-1-NUL")path=std::string("a\0",2)+std::string(PATH_MAX-3,'x');
else if(p=="@UB-MAX-1")path=std::string(PATH_MAX-1,'a');
else if(p!="-")path=unhex(p);
events.clear();access_calls=mkdir_calls=0;last_error=77;errno=7;trace_enabled=true;bool result=swoole_mkdir_recursive(path);int e=errno;trace_enabled=false;
std::cout<<"{\"id\":\""<<id<<"\",\"path_max\":"<<PATH_MAX<<",\"path_hex\":\""<<hex(path)<<"\",\"scenario\":\""<<scenario<<"\",\"result\":"<<(result?"true":"false")<<",\"errno\":"<<e<<",\"last_error\":"<<last_error<<",\"events\":[";
for(size_t i=0;i<events.size();++i){auto&v=events[i];if(i)std::cout<<",";std::cout<<"{\"op\":\""<<v.operation<<"\",\"path_hex\":\""<<hex(v.path)<<"\",\"argument\":"<<v.argument<<",\"return\":"<<v.result<<",\"errno_before\":"<<v.before<<",\"errno_after\":"<<v.after<<"}";}
std::cout<<"]}\n";
}}
