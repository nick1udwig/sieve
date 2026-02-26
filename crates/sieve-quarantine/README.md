# sieve-quarantine

`sieve-quarantine` runs accepted unknown/uncertain commands in sandbox (`bwrap` + `strace`) and emits normalized capability attempts.

## Trace Artifacts

- Per-run directory: `~/.sieve/logs/traces/<run_id>/`
- Always written on successful traced runs:
  - `stdout.log`
  - `stderr.log`
  - `strace*`
  - `report.json`

## Syscall -> Capability Mapping (MVP)

| Syscall family | Capability |
| --- | --- |
| `execve`, `execveat` | `proc.exec` |
| `clone`, `clone3`, `fork`, `vfork` | `proc.exec` (`spawn.<syscall>:pid=<pid|unknown>`) |
| `open/openat/openat2/creat` | `fs.read|write|append` |
| `open*` path ending in `/environ` | `env.read|write` (`proc_environ:pid=<value|unknown>`) |
| `getenv`, `secure_getenv` | `env.read` (`key=<name|unknown>`) |
| `setenv`, `putenv`, `unsetenv` | `env.write` (`key=<name|unknown>`) |
| `connect/socket/sendto/sendmsg/recvfrom/recvmsg/bind/listen/accept/accept4` + `AF_INET/AF_INET6` | `net.connect` |
| same connect family + `AF_UNIX` | `ipc.connect` |
| connect family with unknown socket family | `net.connect` fallback (`family=unknown,address=unknown,port=0`) |

## Normalized Scope Formats

- Net IPv4: `family=af_inet,address=<ip>,port=<u16>`
- Net IPv6: `family=af_inet6,address=<ip6>,port=<u16>`
- IPC Unix socket: `family=af_unix,path=<path|unknown>`
- Environment proc file: `proc_environ:pid=<pid|self|unknown>`

## Known Limits

- Parser is line/heuristic based; not full `strace` grammar.
- Distribution/kernel `strace` formatting differences may reduce extraction quality.
- Unknown address families collapse to generic fallback scope.
- Environment inference is best-effort from trace-visible indicators only.
