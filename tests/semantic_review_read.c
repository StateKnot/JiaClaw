/* Test-only thread-held read on disposable source inode; never linked into production. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#ifndef __APPLE__
#include <dlfcn.h>
#endif

static atomic_int fired;
static ssize_t review_read(int fd, void *buffer, size_t count) {
    const char *source = getenv("JIACLAW_TEST_REVIEW_SOURCE");
    const char *arm = getenv("JIACLAW_TEST_REVIEW_ARM");
    const char *marker = getenv("JIACLAW_TEST_REVIEW_MARKER");
    const char *release = getenv("JIACLAW_TEST_REVIEW_RELEASE");
    struct stat actual, expected, armed;
    int zero = 0;
    if (source && arm && marker && release && stat(arm, &armed) == 0 &&
        fstat(fd, &actual) == 0 && stat(source, &expected) == 0 &&
        S_ISREG(actual.st_mode) && actual.st_dev == expected.st_dev && actual.st_ino == expected.st_ino &&
        atomic_compare_exchange_strong(&fired, &zero, 1)) {
        int out = open(marker, O_CREAT | O_WRONLY | O_EXCL | O_NOFOLLOW, 0600);
        if (out >= 0) { write(out, "read-held\n", 10); close(out); }
        struct timespec delay = {0, 10000000};
        for (int i = 0; i < 3000 && access(release, F_OK) != 0; ++i) nanosleep(&delay, 0);
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
    interpose_read __attribute__((section("__DATA,__interpose"))) = { (const void *)review_read, (const void *)read };
#else
ssize_t read(int fd, void *buffer, size_t count) { return review_read(fd, buffer, count); }
#endif
