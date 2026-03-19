# MerlionOS Syscall Reference

ABI: `int 0x80` — `rax`=number, `rdi`=arg1, `rsi`=arg2, `rdx`=arg3, return in `rax`.

## Process (0-14)

| # | Name | Args | Returns |
|---|------|------|---------|
| 0 | write | buf_ptr, len | bytes_written |
| 1 | exit | code | noreturn |
| 2 | yield | — | 0 |
| 3 | getpid | — | pid |
| 4 | sleep | ticks | 0 |
| 5 | send | channel, byte | 0 |
| 6 | recv | channel | byte |
| 7 | getuid | — | uid |
| 8 | setuid | uid | 0 |
| 9 | getgid | — | gid |
| 10 | setgid | gid | 0 |
| 11 | getgroups | — | count |
| 12 | chmod | path_ptr, mode | 0 |
| 13 | chown | path_ptr, uid_gid | 0 |
| 14 | access | path_ptr, mode | 0 |

## File Operations (100-109)

| # | Name | Args | Returns |
|---|------|------|---------|
| 100 | open | path_ptr, path_len, flags | fd |
| 101 | read | fd, buf_ptr, len | bytes_read |
| 102 | close | fd | 0 |
| 103 | stat | path_ptr, path_len, buf_ptr | 0 |
| 104 | lseek | fd, offset, whence | new_offset |
| 105 | mkdir | path_ptr, path_len | 0 |
| 106 | unlink | path_ptr, path_len | 0 |
| 107 | readdir | path_ptr, path_len, buf_ptr | entry_count |
| 108 | chdir | path_ptr, path_len | 0 |
| 109 | getcwd | buf_ptr, buf_len | len |

## Process Management (110-115)

| # | Name | Args | Returns |
|---|------|------|---------|
| 110 | fork | — | child_pid |
| 111 | exec | path_ptr, path_len | noreturn or -1 |
| 112 | waitpid | pid | exit_code |
| 113 | brk | addr | new_brk |
| 114 | getppid | — | parent_pid |
| 115 | kill | pid, signal | 0 |

## Memory (120-122)

| # | Name | Args | Returns |
|---|------|------|---------|
| 120 | mmap | addr_hint, len, prot | mapped_addr |
| 121 | munmap | addr, len | 0 |
| 122 | mprotect | addr, len, prot | 0 |

## Network (130-136)

| # | Name | Args | Returns |
|---|------|------|---------|
| 130 | socket | domain, type, proto | fd |
| 131 | connect | fd, addr_ptr, addr_len | 0 |
| 132 | sendto | fd, buf_ptr, len | bytes_sent |
| 133 | recvfrom | fd, buf_ptr, len | bytes_received |
| 134 | bind | fd, addr_ptr, addr_len | 0 |
| 135 | listen | fd, backlog | 0 |
| 136 | accept | fd | new_fd |

## Time (140-142)

| # | Name | Args | Returns |
|---|------|------|---------|
| 140 | time | — | seconds_since_boot |
| 141 | nanosleep | ms | 0 |
| 142 | clock_gettime | buf_ptr | 0 |

## Misc (150-152)

| # | Name | Args | Returns |
|---|------|------|---------|
| 150 | ioctl | fd, request, arg | result |
| 151 | pipe | fds_ptr | 0 (writes [read_fd, write_fd]) |
| 152 | dup2 | oldfd, newfd | newfd |

## Libc (160)

| # | Name | Args | Returns |
|---|------|------|---------|
| 160 | printf | fmt_ptr, fmt_len, int_arg | chars_written |

## Dynamic Linking (170-172)

| # | Name | Args | Returns |
|---|------|------|---------|
| 170 | dlopen | name_ptr, name_len | handle |
| 171 | dlsym | handle, name_ptr, name_len | func_addr |
| 172 | dlclose | handle | 0 |

## Signals (180-181)

| # | Name | Args | Returns |
|---|------|------|---------|
| 180 | sigaction | signal, handler_type | 0 |
| 181 | sigreturn | — | 0 |

## Threads & IPC (190-197)

| # | Name | Args | Returns |
|---|------|------|---------|
| 190 | clone | flags, stack_ptr | child_tid |
| 191 | shmget | key, size | shmid |
| 192 | shmat | shmid | addr |
| 193 | shmdt | shmid | 0 |
| 194 | tty_read | buf_ptr, max_len | bytes_read |
| 195 | fwrite | fd, buf_ptr, len | bytes_written |
| 196 | fbwrite | x, y, color | 0 |
| 197 | wget | url_ptr, url_len, buf_ptr | bytes_received |

## Audio & Hardware (200-205)

| # | Name | Args | Returns |
|---|------|------|---------|
| 200 | beep | freq_hz, duration_ms | 0 |
| 201 | play_tone | freq_hz, duration_ms | 0 |
| 202 | disk_read | sector, buf_ptr | 512 or -1 |
| 203 | disk_write | sector, buf_ptr | 0 or -1 |
| 204 | cpuinfo | buf_ptr, max_len | bytes_written |
| 205 | usb_list | buf_ptr, max_len | bytes_written |

## User Programs

| Name | Description |
|------|-------------|
| hello | Print greeting, exit |
| cat-test | File syscall test |
| qfc-test | QFC miner test |
| counter | Tick 3 times with yield |
| getpid | Get and print PID |
| syscall-test | Test syscall return values |
| open-test | Test SYS_OPEN |
| exec-test | Test SYS_EXEC |
| malloc-test | malloc + memset test |
| printf-test | printf formatting test |
| string-test | strlen + strcmp test |
| libc-demo | Comprehensive libc demo |
| dynlink-test | dlopen/dlsym/dlclose |
| cat | Read /proc/version |
| echo | Print message |
| wc | Count bytes in file |
| ls | List root directory |
| init | PID 1 init process |
| ush | Micro-shell (fork+exec) |
| fwrite-test | Write file to VFS |
| paint | Draw on framebuffer |
| wget-user | HTTP fetch from Ring 3 |
| pkg-install | Install package to /bin |
| test-suite | Syscall validation tests |
| beep | Play A4-A5-A6 melody |
