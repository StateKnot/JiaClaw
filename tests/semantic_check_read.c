/* Test-only read interposer on disposable files; never linked into JiaClaw. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>
#ifndef __APPLE__
#include <dlfcn.h>
#endif

static atomic_int fired;
static ssize_t check_read(int fd, void *buffer, size_t count) {
    const char *source = getenv("JIACLAW_TEST_CHECK_SOURCE");
    const char *marker = getenv("JIACLAW_TEST_CHECK_MARKER");
    struct stat actual, expected;
    int zero = 0;
    if (source && marker && fstat(fd, &actual) == 0 && stat(source, &expected) == 0 &&
        S_ISREG(actual.st_mode) && actual.st_dev == expected.st_dev && actual.st_ino == expected.st_ino &&
        atomic_compare_exchange_strong(&fired, &zero, 1)) {
        int out = open(marker, O_CREAT | O_WRONLY | O_EXCL, 0600);
        if (out >= 0) { write(out, "read-held\n", 10); close(out); }
        raise(SIGSTOP);
    }
#ifdef __APPLE__
    return read(fd, buffer, count);
#else
    static ssize_t (*original)(int, void *, size_t);
    if (!original) original = dlsym(RTLD_NEXT, "read");
    return original(fd, buffer, count);
#endif
}
#ifdef __APPLE__
__attribute__((used)) static struct { const void *replacement; const void *original; }
    interpose_read __attribute__((section("__DATA,__interpose"))) = { (const void *)check_read, (const void *)read };
#else
ssize_t read(int fd, void *buffer, size_t count) { return check_read(fd, buffer, count); }
#endif
