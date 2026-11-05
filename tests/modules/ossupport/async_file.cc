#include "swoole_file.h"
#include "swoole_coroutine_api.h"
#include <cassert>
#include <iostream>
#include <vector>
#include <unistd.h>
static int next_fd=100,latest_fd=-1;
static bool force_zero=false;
static const swoole::AsyncFile *closing=nullptr;
static std::vector<std::string> trace;
static void event(const char *name,int fd,long result,int error) {
    trace.push_back(std::string(name)+":"+std::to_string(fd)+":"+std::to_string(result)+":"+std::to_string(error));
    errno=error;
}
extern "C" {
int swoole_coroutine_open(const char *path,int flags,mode_t mode) {
    int raw=::open(path,flags,mode),fd=raw;
    if(raw>=0){fd=force_zero?::dup2(raw,0) : ::fcntl(raw,F_DUPFD_CLOEXEC,next_fd++);if(raw!=fd)::close(raw);}
    latest_fd=fd;event("open",fd,fd,errno);return fd;
}
int swoole_coroutine_close(int fd) {
    if(closing)assert(closing->ready());
    int result=::close(fd),error=errno;
    if(closing)assert(closing->ready());
    event("close",fd,result,error);return result;
}
ssize_t swoole_coroutine_read(int fd,void *buffer,size_t size) {
    ssize_t result=::read(fd,buffer,size);event("read",fd,result,errno);return result;
}
ssize_t swoole_coroutine_write(int fd,const void *buffer,size_t size) {
    ssize_t result=::write(fd,buffer,size);event("write",fd,result,errno);return result;
}
int swoole_coroutine_fsync(int fd) {int result=::fsync(fd);event("sync",fd,result,errno);return result;}
int swoole_coroutine_ftruncate(int fd,off_t size) {int result=::ftruncate(fd,size);event("truncate",fd,result,errno);return result;}
int swoole_coroutine_fstat(int fd,struct stat *info) {int result=::fstat(fd,info);event("stat",fd,result,errno);return result;}
off_t swoole_coroutine_lseek(int fd,off_t offset,int whence) {off_t result=::lseek(fd,offset,whence);event("seek",fd,result,errno);return result;}
}
int main(int argc,char **argv) {
    assert(argc==2);const std::string path=std::string(argv[1])+"/async-owner";
    errno=EINTR;
    auto *file=new swoole::AsyncFile(path+std::string("\0tail",5),O_CREAT|O_RDWR|O_TRUNC,0600);
    assert(file->ready());const int first=latest_fd;
    assert(file->write("abc",3)==3);assert(file->get_offset()==3);assert(file->set_offset(0)==0);
    char buffer[4]{};assert(file->read(buffer,3)==3);assert(std::string(buffer,3)=="abc");
    assert(file->sync());assert(file->truncate(2));struct stat info{};assert(file->stat(&info));assert(info.st_size==2);
    auto *copy=new swoole::AsyncFile(*file);assert(copy->ready());const auto *same=copy;*copy=*same;
    closing=file;assert(file->open(path,O_RDWR,0600));closing=nullptr;
    assert(latest_fd!=first);errno=0;assert(copy->write("z",1)==-1);assert(errno==EBADF);
    assert(file->write("z",1)==1);*copy=*file;
    closing=file;delete file;closing=nullptr;
    assert(copy->ready());closing=copy;assert(!copy->close());delete copy;closing=nullptr;
    // The zero descriptor is ready but open's fd>0 return deliberately rejects it.
    int saved=::fcntl(0,F_DUPFD_CLOEXEC,1000);assert(saved>=0);force_zero=true;
    auto *zero=new swoole::AsyncFile(path,O_RDWR,0600);assert(latest_fd==0&&zero->ready());
    closing=zero;assert(!zero->open(path,O_RDWR,0600));assert(latest_fd==0&&zero->ready());delete zero;closing=nullptr;
    assert(::dup2(saved,0)==0);assert(::close(saved)==0);force_zero=false;
    auto *missing=new swoole::AsyncFile(path+"/missing",O_RDONLY,0);assert(!missing->ready());assert(!missing->close());delete missing;
    assert(::unlink(path.c_str())==0);
    std::cout<<"{\"events\":[";
    for(size_t i=0;i<trace.size();++i){if(i)std::cout<<',';std::cout<<'"'<<trace[i]<<'"';}
    std::cout<<"],\"passed\":true}\n";
}
