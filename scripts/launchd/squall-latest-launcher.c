// launchd entry point for the weekly squall-latest job.
//
// Why this exists: the repo lives in ~/Documents, which macOS privacy protection (TCC)
// blocks for anything launchd starts ("Operation not permitted"). Full Disk Access is
// the only fix. Granting it to /bin/zsh would hand it to every zsh script launchd ever
// runs, so instead this launcher gets the grant. It can run exactly one script, fixed at
// compile time. TCC attributes the spawned zsh (and its git/cargo/claude children) to
// the launcher, the responsible process, so they inherit its access.
//
// It must posix_spawn, not exec: exec would turn this process into /bin/zsh, and TCC
// would then judge it as zsh.
//
// Build + install (rebuilding changes the code signature, so re-grant Full Disk Access):
//   clang -O2 -o ~/.local/bin/squall-latest-launcher \
//     -DSCRIPT="\"$PWD/scripts/squall-latest-weekly.sh\"" scripts/launchd/squall-latest-launcher.c
#include <spawn.h>
#include <stdio.h>
#include <sys/wait.h>

#ifndef SCRIPT
#error "build with -DSCRIPT=\"/abs/path/to/squall-latest-weekly.sh\""
#endif

extern char **environ;

int main(void) {
    char *argv[] = {"/bin/zsh", "-l", SCRIPT, NULL};
    pid_t pid;
    int rc = posix_spawn(&pid, argv[0], NULL, NULL, argv, environ);
    if (rc != 0) {
        fprintf(stderr, "squall-latest-launcher: spawn failed (%d)\n", rc);
        return 127;
    }
    int status;
    if (waitpid(pid, &status, 0) < 0) {
        perror("squall-latest-launcher: waitpid");
        return 1;
    }
    return WIFEXITED(status) ? WEXITSTATUS(status) : 1;
}
