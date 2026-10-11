/* Test-only OS interposer; never linked into JiaClaw. Real fsync/SIGKILL
 * boundaries on disposable local files, not power-loss certification. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#ifndef __APPLE__
#include <dlfcn.h>
#endif

static int exists(const char *path) { struct stat s; return lstat(path, &s) == 0; }
static int equal_files(const char *left, const char *right) {
    FILE *a = fopen(left, "rb"), *b = fopen(right, "rb");
    if (!a || !b) { if (a) fclose(a); if (b) fclose(b); return 0; }
    int x, y, same = 1;
    do { x = fgetc(a); y = fgetc(b); if (x != y) { same = 0; break; } } while (x != EOF);
    fclose(a); fclose(b); return same;
}
static int selected(int fd) {
    const char *workspace = getenv("JIACLAW_TEST_INSTALL_WORKSPACE");
    const char *id = getenv("JIACLAW_TEST_INSTALL_ID");
    const char *phase = getenv("JIACLAW_TEST_INSTALL_PHASE");
    if (!workspace || !id || !phase) return 0;
    char path[4096], op[4096], intent[4096], stage[4096], receipt[4096], policy[4096], proposed[4096], skills[4096], installed[4096];
#ifdef __APPLE__
    if (fcntl(fd, F_GETPATH, path) != 0) return 0;
#else
    char link[64]; snprintf(link, sizeof link, "/proc/self/fd/%d", fd);
    ssize_t length = readlink(link, path, sizeof path - 1); if (length < 0) return 0; path[length] = 0;
#endif
    snprintf(op, sizeof op, "%s/.jiaclaw-skill-installs/%s", workspace, id);
    snprintf(intent, sizeof intent, "%s/intent.json", op);
    snprintf(stage, sizeof stage, "%s/stage", op);
    snprintf(receipt, sizeof receipt, "%s/receipt.json", op);
    snprintf(policy, sizeof policy, "%s/skills.lock.json", workspace);
    snprintf(proposed, sizeof proposed, "%s/proposed-policy.json", op);
    snprintf(skills, sizeof skills, "%s/skills", workspace);
    snprintf(installed, sizeof installed, "%s/newskill", skills);
    if (!strcmp(phase, "preparing")) return !strcmp(path, op) && !exists(intent);
    if (!exists(intent)) return 0;
    if (!strcmp(phase, "stage-temp")) return !strncmp(path, stage, strlen(stage)) && path[strlen(stage)] == '/' && strstr(path + strlen(stage), ".jiaclaw-memory-") != NULL;
    if (!strcmp(phase, "intent")) return !strcmp(path, op) && !exists(stage);
    if (!strcmp(phase, "stage")) return !strcmp(path, stage);
    if (!strcmp(phase, "policy")) return !strcmp(path, workspace) && equal_files(policy, proposed) && !exists(installed);
    if (!strcmp(phase, "files")) return !strcmp(path, skills) && exists(installed);
    if (!strcmp(phase, "receipt")) return !strcmp(path, op) && exists(receipt);
    return 0;
}
static int after_sync(int fd, int result) {
    if (result == 0 && selected(fd)) {
        const char *mode = getenv("JIACLAW_TEST_INSTALL_MODE");
        if (mode && !strcmp(mode, "error")) { errno = EIO; return -1; }
        const char *marker = getenv("JIACLAW_TEST_INSTALL_MARKER");
        if (marker) { int out = open(marker, O_CREAT | O_WRONLY | O_EXCL, 0600); if (out >= 0) { write(out, "held\n", 5); close(out); } }
        raise(SIGSTOP);
    }
    return result;
}
static int installer_fsync(int fd) {
#ifdef __APPLE__
    int result = fsync(fd);
#else
    static int (*real_fsync)(int);
    if (!real_fsync) real_fsync = dlsym(RTLD_NEXT, "fsync");
    int result = real_fsync(fd);
#endif
    return after_sync(fd, result);
}
#ifdef __APPLE__
/* Rust sync_all uses F_FULLFSYNC on macOS, not necessarily fsync. All
 * commands used by the subprocess keep their real argument conventions. */
static int installer_fcntl(int fd, int cmd, ...) {
    if (cmd == F_FULLFSYNC) return after_sync(fd, fcntl(fd, cmd));
    if (cmd == F_GETFD || cmd == F_GETFL || cmd == F_GETOWN) return fcntl(fd, cmd);
    va_list args; va_start(args, cmd); int result;
    if (cmd == F_DUPFD || cmd == F_DUPFD_CLOEXEC || cmd == F_SETFD || cmd == F_SETFL || cmd == F_SETOWN || cmd == F_NOCACHE) {
        int value = va_arg(args, int); result = fcntl(fd, cmd, value);
    } else { void *value = va_arg(args, void *); result = fcntl(fd, cmd, value); }
    va_end(args); return result;
}
__attribute__((used)) static struct { const void *replacement; const void *original; }
    interpose_fsync __attribute__((section("__DATA,__interpose"))) = { (const void *)installer_fsync, (const void *)fsync };
__attribute__((used)) static struct { const void *replacement; const void *original; }
    interpose_fcntl __attribute__((section("__DATA,__interpose"))) = { (const void *)installer_fcntl, (const void *)fcntl };
#else
int fsync(int fd) { return installer_fsync(fd); }
#endif
