/*
   +----------------------------------------------------------------------+
   | Copyright (c) The PHP Group                                          |
   +----------------------------------------------------------------------+
   | This source file is subject to version 3.01 of the PHP license,      |
   | that is bundled with this package in the file LICENSE, and is        |
   | available through the world-wide-web at the following url:           |
   | https://www.php.net/license/3_01.txt                                 |
   | If you did not receive a copy of the PHP license and are unable to   |
   | obtain it through the world-wide-web, please send a note to          |
   | license@php.net so we can mail you a copy immediately.               |
   +----------------------------------------------------------------------+
   | Author: Wez Furlong <wez@thebrainroom.com>                           |
   +----------------------------------------------------------------------+
 */

#include "thirdparty/php/standard/proc_open.h"
#include "swoole_rs_phpruntime.h"

using swoole::Coroutine;
using swoole::coroutine::System;

#ifdef HAVE_SYS_WAIT_H
#include <sys/wait.h>
#endif

#ifdef HAVE_FCNTL_H
#include <fcntl.h>
#endif

#ifdef HAVE_PTY_H
#include <pty.h>
#elif defined(__FreeBSD__)
/* FreeBSD defines `openpty` in <libutil.h> */
#include <libutil.h>
#elif defined(__NetBSD__) || defined(__DragonFly__)
/* On recent NetBSD/DragonFlyBSD releases the emalloc, estrdup ... calls had been introduced in libutil */
#if defined(__NetBSD__)
#include <sys/termios.h>
#else
#include <termios.h>
#endif
extern int openpty(int *, int *, char *, struct termios *, struct winsize *);
#elif defined(__sun)
#include <termios.h>
#else
/* Mac OS X (and some BSDs) define `openpty` in <util.h> */
#include <util.h>
#endif

static int le_proc_open;
static const char *le_proc_name = "process/coroutine";

SW_EXTERN_C_BEGIN
/* crates/phpruntime (proc.rs): the environment and argv `proc_open` builds before it forks,
 * in request memory freed here (`_php_free_envp`, `efree_argv`). */
void swr_proc_envp(HashTable *environment, char **envp, char ***envarray);
/* proc_co_rsrc_dtor's wait (proc.rs): a still-running child reaped into `wstatus`. */
void swr_proc_reap(bool running, pid_t child, int *wstatus);
SW_EXTERN_C_END

/* {{{ _php_array_to_envp
 * Process the `environment` argument to `proc_open`
 * Convert into data structures which can be passed to underlying OS APIs like `exec` on POSIX */
static sw_php_process_env _php_array_to_envp(zval *environment) {
    sw_php_process_env env;
    memset(&env, 0, sizeof(env));
    if (environment) {
        swr_proc_envp(Z_ARRVAL_P(environment), &env.envp, &env.envarray);
    }
    return env;
}
/* }}} */

/* {{{ _php_free_envp
 * Free the structures allocated by `_php_array_to_envp` */
static void _php_free_envp(sw_php_process_env env) {
    if (env.envarray) {
        efree(env.envarray);
    }
    if (env.envp) {
        efree(env.envp);
    }
}
/* }}} */

static void proc_co_rsrc_dtor(zend_resource *rsrc) {
    sw_php_process_handle *proc = (sw_php_process_handle *) rsrc->ptr;
    int wstatus = 0;

    /* Close all handles to avoid a deadlock */
    for (int i = 0; i < proc->npipes; i++) {
        if (proc->pipes[i] != NULL) {
            GC_DELREF(proc->pipes[i]);
            zend_list_close(proc->pipes[i]);
            proc->pipes[i] = NULL;
        }
    }

    swr_proc_reap(proc->running, proc->child, &wstatus);
    if (proc->wstatus) {
        *proc->wstatus = wstatus;
    }

    _php_free_envp(proc->env);
    efree(proc->pipes);
    zend_string_release_ex(proc->command, false);
    efree(proc);
}
/* }}} */

/* {{{ PHP_MINIT_FUNCTION(proc_open) */
void swoole_proc_open_init(int module_number) {
    le_proc_open = zend_register_list_destructors_ex(proc_co_rsrc_dtor, NULL, le_proc_name, module_number);
}
/* }}} */

/* {{{ The record behind a `proc_open` resource, for crates/phpruntime's proc_terminate,
 * proc_close and proc_get_status (proc.rs), which hold it as a pointer and read it
 * through a pointer to that. */
SW_EXTERN_C_BEGIN
/* `zproc`'s record, or NULL once zend_fetch_resource has thrown (a resource of another
 * type). */
sw_php_process_handle *swr_proc_fetch(zval *zproc) {
    return (sw_php_process_handle *) zend_fetch_resource(Z_RES_P(zproc), le_proc_name, le_proc_open);
}

pid_t swr_proc_child(sw_php_process_handle *const *proc) {
    return (*proc)->child;
}

zend_string *swr_proc_command(sw_php_process_handle *const *proc) {
    return (*proc)->command;
}

void swr_proc_set_running(sw_php_process_handle *const *proc, bool running) {
    (*proc)->running = running;
}

/* Closes `zproc`'s resource, which frees `proc`; its destructor reaps a running child
 * into the wait status returned. */
int swr_proc_close(sw_php_process_handle *proc, zval *zproc) {
    int wstatus = 0;
    proc->wstatus = &wstatus;
    zend_list_close(Z_RES_P(zproc));
    return wstatus;
}
SW_EXTERN_C_END
/* }}} */

#define close_descriptor(fd) close(fd)

/* Determines the type of a descriptor item. */
typedef enum _descriptor_type { DESCRIPTOR_TYPE_STD, DESCRIPTOR_TYPE_PIPE, DESCRIPTOR_TYPE_SOCKET } descriptor_type;

/* One instance of this struct is created for each item in `$descriptorspec` argument to `proc_open`
 * They are used within `proc_open` and freed before it returns */
typedef struct _descriptorspec_item {
    int index; /* desired FD # in child process */
    descriptor_type type;
    php_file_descriptor_t childend;  /* FD # opened for use in child
                                      * (will be copied to `index` in child) */
    php_file_descriptor_t parentend; /* FD # opened for use in parent
                                      * (for pipes only; will be 0 otherwise) */
    int mode_flags;                  /* mode for opening FDs: r/o, r/w, binary (on Win32), etc */
} descriptorspec_item;



static descriptorspec_item *alloc_descriptor_array(HashTable *descriptorspec) {
    uint32_t ndescriptors = zend_hash_num_elements(descriptorspec);
    return (descriptorspec_item *) ecalloc(sizeof(descriptorspec_item), ndescriptors);
}

SW_EXTERN_C_BEGIN
bool swr_proc_descriptors(HashTable *spec, descriptorspec_item *descriptors, int pty[2], int *ndesc);

/* For proc.rs: item `i`'s child fd and its setup (type, mode, parent end, child end). */
void swr_proc_descriptor_set(descriptorspec_item *descriptors, int i, int index, const int setup[4]) {
    descriptorspec_item *desc = &descriptors[i];
    desc->index = index;
    desc->type = (descriptor_type) setup[0];
    desc->mode_flags = setup[1];
    desc->parentend = setup[2];
    desc->childend = setup[3];
}

/* For proc.rs: the child end of the first of `count` descriptors whose index is `target`,
 * or -1. */
int swr_proc_descriptor_child_of(const descriptorspec_item *descriptors, int count, int target) {
    for (int i = 0; i < count; i++) {
        if (descriptors[i].index == target) {
            return descriptors[i].childend;
        }
    }
    return PHP_INVALID_FD;
}

/* For proc.rs: a 'file' descriptor, `path` opened in `mode` through its stream wrapper and
 * released as a descriptor; false once it reported. */
bool swr_proc_open_file(zend_string *path, zend_string *mode, int *fd) {
    php_stream *stream =
        php_stream_open_wrapper(ZSTR_VAL(path), ZSTR_VAL(mode), REPORT_ERRORS | STREAM_WILL_CAST, NULL);
    if (stream == NULL) {
        return false;
    }
    php_socket_t sock;
    if (php_stream_cast(stream, PHP_STREAM_CAST_RELEASE | PHP_STREAM_AS_FD, (void **) &sock, REPORT_ERRORS) ==
        FAILURE) {
        return false;
    }
    *fd = (int) sock;
    return true;
}

/* For proc.rs: a stream resource's descriptor (set_proc_descriptor_from_resource); false
 * once it threw or reported. */
bool swr_proc_stream_fd(zval *resource, int *fd) {
    php_stream *stream = (php_stream *) zend_fetch_resource(Z_RES_P(resource), "stream", php_file_le_stream());
    if (stream == NULL) {
        return false;
    }
    php_socket_t sock;
    if (php_stream_cast(stream, PHP_STREAM_AS_FD, (void **) &sock, REPORT_ERRORS) == FAILURE) {
        return false;
    }
    *fd = (int) sock;
    return true;
}
SW_EXTERN_C_END




SW_EXTERN_C_BEGIN
/* What the forked child execs (proc.rs passes it back untouched). */
typedef struct {
    const char *cwd;
    zend_string *command;
    char **argv;
    char **envarray;
} sw_proc_exec;

bool swr_proc_fork(descriptorspec_item *descriptors, int ndesc, const sw_proc_exec *exec, pid_t *child);

/* For proc.rs: descriptor `i`'s index, type, child end, parent end and mode flags. */
void swr_proc_descriptor_at(const descriptorspec_item *descriptors, int i, int out[5]) {
    out[0] = descriptors[i].index;
    out[1] = (int) descriptors[i].type;
    out[2] = descriptors[i].childend;
    out[3] = descriptors[i].parentend;
    out[4] = descriptors[i].mode_flags;
}

/* For proc.rs's parent side: a pipe's parent end as a stream, which takes `fd`. */
php_stream *swr_proc_pipe_stream(int fd, const char *mode) {
    return php_swoole_create_stream_from_pipe(fd, mode, NULL STREAMS_CC);
}

void swr_proc_parent(descriptorspec_item *descriptors, int ndesc, sw_php_process_handle *const *proc, zval *pipes);

/* For proc.rs's parent side: `stream` (or none) as the record's pipe `i`, and in
 * `pipes` under `index`. */
void swr_proc_attach(sw_php_process_handle *const *handle, int i, php_stream *stream, zval *pipes, int index) {
    sw_php_process_handle *proc = *handle;
    if (!stream) {
        proc->pipes[i] = NULL;
        return;
    }
    zval retfp;

    /* nasty hack; don't copy it */
    stream->flags |= PHP_STREAM_FLAG_NO_SEEK;

    php_stream_to_zval(stream, &retfp);
    add_index_zval(pipes, index, &retfp);

    proc->pipes[i] = Z_RES(retfp);
    Z_ADDREF(retfp);
}

/* For proc.rs, in the child: returns only when the exec failed. */
void swr_proc_exec(const sw_proc_exec *exec) {
    if (exec->cwd) {
        php_ignore_value(chdir(exec->cwd));
    }
    if (exec->argv) {
        /* execvpe() is non-portable, use environ instead. */
        if (exec->envarray) {
            environ = exec->envarray;
        }
        execvp(ZSTR_VAL(exec->command), exec->argv);
    } else if (exec->envarray) {
        execle("/bin/sh", "sh", "-c", ZSTR_VAL(exec->command), NULL, exec->envarray);
    } else {
        execl("/bin/sh", "sh", "-c", ZSTR_VAL(exec->command), NULL);
    }
}
SW_EXTERN_C_END

static void close_all_descriptors(descriptorspec_item *descriptors, int ndesc) {
    for (int i = 0; i < ndesc; i++) {
        close_descriptor(descriptors[i].childend);
        if (descriptors[i].parentend) close_descriptor(descriptors[i].parentend);
    }
}

static void efree_argv(char **argv) {
    if (argv) {
        char **arg = argv;
        while (*arg != NULL) {
            efree(*arg);
            arg++;
        }
        efree(argv);
    }
}

/* {{{ Execute a command, with specified files used for input/output */
SW_EXTERN_C_BEGIN
/* The rest of proc_open once proc.rs parsed its arguments, built the command, argv and
 * environment, and ran its checks: the descriptor array, the walk over $descriptorspec,
 * the fork, and the parent's streams into `pipes`, leaving the process resource or false
 * in `return_value`. It takes `argv` and the environment; `command_str` and `cwd_str`
 * stay the caller's. */
static void proc_open_spawn(zend_string *command_str,
                         char **argv,
                         HashTable *descriptorspec,
                         zval *pipes,
                         zend_string *cwd_str,
                         char *envp,
                         char **envarray,
                         zval *return_value) {
    const char *cwd = cwd_str ? ZSTR_VAL(cwd_str) : NULL;
    sw_php_process_env env;
    int ndesc = 0;
    descriptorspec_item *descriptors = NULL;
    int pty_master_fd = -1, pty_slave_fd = -1;
    php_process_id_t child;
    sw_php_process_handle *proc;

    env.envp = envp;
    env.envarray = envarray;

    descriptors = alloc_descriptor_array(descriptorspec);

    /* Walk the descriptor spec and set up files/pipes */
    {
        /* Its own block: the gotos to exit_fail don't cross its initialization. */
        int pty[2] = {pty_master_fd, pty_slave_fd};
        bool walked = swr_proc_descriptors(descriptorspec, descriptors, pty, &ndesc);
        pty_master_fd = pty[0];
        pty_slave_fd = pty[1];
        if (!walked) {
            goto exit_fail;
        }
    }

    {
        sw_proc_exec exec = {cwd, command_str, argv, env.envarray};
        if (!swr_proc_fork(descriptors, ndesc, &exec, &child)) {
            close_all_descriptors(descriptors, ndesc);
            goto exit_fail;
        }
    }

    /* We forked/spawned and this is the parent */

    pipes = zend_try_array_init(pipes);
    if (!pipes) {
        goto exit_fail;
    }

    proc = (sw_php_process_handle *) emalloc(sizeof(sw_php_process_handle));
    proc->command = zend_string_copy(command_str);
    proc->wstatus = nullptr;
    proc->running = true;
    proc->pipes = (zend_resource **) emalloc(sizeof(zend_resource *) * ndesc);
    proc->npipes = ndesc;
    proc->child = child;
    proc->env = env;

    /* Clean up all the child ends and then open streams on the parent
     *   ends, where appropriate (proc.rs) */
    swr_proc_parent(descriptors, ndesc, &proc, pipes);

    if (1) {
        RETVAL_RES(zend_register_resource(proc, le_proc_open));
    } else {
    exit_fail:
        _php_free_envp(env);
        RETVAL_FALSE;
    }

    efree_argv(argv);
#ifdef HAVE_OPENPTY
    if (pty_master_fd != -1) {
        close(pty_master_fd);
    }
    if (pty_slave_fd != -1) {
        close(pty_slave_fd);
    }
#endif
    if (descriptors) {
        efree(descriptors);
    }
}

/* For proc.rs: proc_open_spawn inside zend_try, so a bailout (a warning's error handler)
 * returns to Rust, which resumes it once its own values are dropped. True when it bailed. */
bool swr_proc_open_spawn(zend_string *command_str,
                         char **argv,
                         HashTable *descriptorspec,
                         zval *pipes,
                         zend_string *cwd_str,
                         char *envp,
                         char **envarray,
                         zval *return_value) {
    volatile bool bailed = false;
    zend_try {
        proc_open_spawn(command_str, argv, descriptorspec, pipes, cwd_str, envp, envarray, return_value);
    }
    zend_catch {
        bailed = true;
    }
    zend_end_try();
    return bailed;
}
SW_EXTERN_C_END
/* }}} */
