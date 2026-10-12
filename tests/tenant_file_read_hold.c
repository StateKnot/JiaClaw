/* Disposable acceptance interposer: actual bounded reader inode only. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#ifndef __APPLE__
#include <dlfcn.h>
#endif
static atomic_int reads;
static ssize_t held_read(int fd, void *buffer, size_t count) {
    const char *source=getenv("JIACLAW_TEST_FILE_SOURCE"), *arm=getenv("JIACLAW_TEST_FILE_ARM");
    const char *marker=getenv("JIACLAW_TEST_FILE_MARKER"), *release=getenv("JIACLAW_TEST_FILE_RELEASE");
    struct stat actual, expected, armed;
    if (source && arm && marker && release && stat(arm,&armed)==0 &&
        fstat(fd,&actual)==0 && stat(source,&expected)==0 &&
        S_ISREG(actual.st_mode) && actual.st_dev==expected.st_dev && actual.st_ino==expected.st_ino) {
        int n=atomic_fetch_add(&reads,1)+1;
        if (n<=8) {
            char path[4096];snprintf(path,sizeof(path),"%s-%d",marker,n);
            int out=open(path,O_CREAT|O_WRONLY|O_EXCL|O_NOFOLLOW,0600);
            if(out>=0){write(out,"read-held\n",10);close(out);}
            struct timespec delay={0,10000000};
            for(int i=0;i<3000 && access(release,F_OK)!=0;i++)nanosleep(&delay,0);
        }
    }
#ifdef __APPLE__
    return read(fd,buffer,count);
#else
    static ssize_t (*original)(int,void*,size_t);
    if(!original)original=dlsym(RTLD_NEXT,"read");
    return original(fd,buffer,count);
#endif
}
#ifdef __APPLE__
__attribute__((used)) static struct {const void *replacement;const void *original;}
    interpose_read __attribute__((section("__DATA,__interpose")))={(const void*)held_read,(const void*)read};
#else
ssize_t read(int fd,void *buffer,size_t count){return held_read(fd,buffer,count);}
#endif
