# Tuwunel for FreeBSD

Tuwunel builds and runs on FreeBSD, but FreeBSD is not a supported platform.
Nothing in the CI matrix builds it, no release artifact is published for it, and
there is no port or package. This page records a build that was verified end to
end, and the handful of things that differ from the platforms we do ship.

Verified against 1.8.3 on FreeBSD 15.1-RELEASE, `aarch64`. The resulting server
opens its database, answers the client and federation version endpoints,
registers an account, creates a room, sends and reads back a message, and exits
cleanly on `SIGTERM`.

Contributions for getting Tuwunel into ports are welcome.


## Toolchain

```sh
pkg install rust cmake git llvm21 pkgconf gmake curl
```

`rust` supplies `cargo` and `rustc`; FreeBSD 15.1 carries 1.96.1, which is at or
above the `rust-version` the workspace declares. `llvm21` is needed for
`libclang.so`, which `rust-librocksdb-sys` runs bindgen against; the base system
clang does not ship it. Point the build at it with `LIBCLANG_PATH`.

This is the set that was verified rather than a minimal one. `curl` is only used
by the checks further down this page.


## Features

The default feature set includes `io_uring` and `systemd`, both of which are
Linux only. Every FreeBSD build therefore has to opt out of the defaults and
name its features explicitly:

```
brotli_compression
element_hacks
gzip_compression
jemalloc
jemalloc_conf
media_thumbnail
release_max_log_level
url_preview
zstd_compression
```

That is the default set with `io_uring` and `systemd` removed.


## RocksDB on arm64

On `aarch64` the RocksDB build fails out of the box:

```
rocksdb/util/crc32c_arm64.cc:60:16: error: use of undeclared identifier 'AT_HWCAP'
   60 |   elf_aux_info(AT_HWCAP, &auxv, sizeof(auxv));
```

`crc32c_arm64.cc` carries include blocks for `__APPLE__` and `__OpenBSD__` but
none for `__FreeBSD__`, while its FreeBSD branch calls `elf_aux_info(AT_HWCAP,
...)`. `elf_aux_info` is declared in `<sys/auxv.h>` and `AT_HWCAP` in
`<sys/elf_common.h>`, and neither header is reached on this platform.

Supplying both to the C++ compile is enough. `<sys/elf_common.h>` is not
self contained, so `<sys/types.h>` has to precede it:

```sh
export CXXFLAGS="-include sys/types.h -include sys/elf_common.h -include sys/auxv.h"
```

This affects `aarch64` only. `crc32c_arm64.cc` is not compiled on `amd64`, so an
`amd64` build needs none of this.

With the headers in place the runtime check works as intended and RocksDB
selects the hardware CRC32C and PMULL paths when the CPU reports them.


## Building

```sh
export LIBCLANG_PATH=/usr/local/llvm21/lib
export CXXFLAGS="-include sys/types.h -include sys/elf_common.h -include sys/auxv.h"

cargo build --release -p tuwunel --no-default-features \
    --features brotli_compression,element_hacks,gzip_compression,jemalloc,jemalloc_conf,media_thumbnail,release_max_log_level,url_preview,zstd_compression
```

A cold build took about 17 minutes on 8 jobs and produced a 79 MB binary. The
result is dynamically linked, and against base system libraries only:

```console
$ ldd target/release/tuwunel
	libc++.so.1 => /lib/libc++.so.1
	libcxxrt.so.1 => /lib/libcxxrt.so.1
	libthr.so.3 => /lib/libthr.so.3
	libgcc_s.so.1 => /lib/libgcc_s.so.1
	libc.so.7 => /lib/libc.so.7
	libm.so.5 => /lib/libm.so.5
	libsys.so.7 => /lib/libsys.so.7
```

Nothing from `pkg` is needed at run time, so the binary can be copied to a host
that has no build toolchain installed.


## Running

Configuration is no different from any other platform; see
[Configuration](../configuration.md). A minimal file to prove the build:

```toml
[global]
server_name = "example.com"
address = "0.0.0.0"
port = 8008
database_path = "/var/db/tuwunel"
```

```sh
tuwunel -c /usr/local/etc/tuwunel/tuwunel.toml
```

### The jemalloc notice

Every invocation, `--version` included, prints one line to standard error:

```
<jemalloc>: option background_thread currently supports pthread only
```

Tuwunel compiles a `malloc_conf` string into the binary that asks for
`background_thread:true`. FreeBSD's jemalloc does not implement that option and
says so. The option is ignored and nothing else changes, so the notice is
cosmetic. Building without the `jemalloc_conf` feature silences it, at the cost
of the rest of the tuned allocator configuration.


## Running under rc.d

Create an account for the service, install the binary, and give it a database
directory:

```sh
pw groupadd tuwunel
pw useradd tuwunel -g tuwunel -d /nonexistent -s /usr/sbin/nologin
install -m 755 target/release/tuwunel /usr/local/bin/tuwunel
mkdir -p /usr/local/etc/tuwunel /var/db/tuwunel
chown tuwunel:tuwunel /var/db/tuwunel
```

Save this as `/usr/local/etc/rc.d/tuwunel` and `chmod 555` it:

```sh
#!/bin/sh
#
# PROVIDE: tuwunel
# REQUIRE: LOGIN NETWORKING
# KEYWORD: shutdown

. /etc/rc.subr

name="tuwunel"
rcvar="tuwunel_enable"

load_rc_config $name

: ${tuwunel_enable:="NO"}
: ${tuwunel_runas:="tuwunel"}
: ${tuwunel_rungroup:="tuwunel"}
: ${tuwunel_config:="/usr/local/etc/tuwunel/tuwunel.toml"}

pidfile="/var/run/${name}/${name}.pid"
procname="/usr/local/bin/tuwunel"
command="/usr/sbin/daemon"
command_args="-f -S -T ${name} -p ${pidfile} -u ${tuwunel_runas} ${procname} -c ${tuwunel_config}"

start_precmd="tuwunel_precmd"
tuwunel_precmd()
{
	install -d -o "${tuwunel_runas}" -g "${tuwunel_rungroup}" -m 755 "/var/run/${name}"
}

run_rc_command "$1"
```

```sh
sysrc tuwunel_enable=YES
service tuwunel start
```

Two details in that script are worth keeping if you rewrite it.

The account variable is `tuwunel_runas` and not `tuwunel_user`, because
`rc.subr` gives `${name}_user` its own meaning: it drops privileges itself
before running the command, after which `daemon(8)` is no longer root and its
own `-u` fails with `initgroups(tuwunel, 1001): Operation not permitted`. Only
one of the two should be doing the drop.

The pidfile lives in `/var/run/tuwunel/` rather than `/var/run/`, because
`daemon(8)` drops privileges before it writes the pidfile. Writing straight to
`/var/run/` fails with `Permission denied`, so `start_precmd` creates a
directory the account owns.

Output goes to syslog under the `tuwunel` tag. Point a `local` facility at a
file through `syslog.conf` if you want it separated out.


## Differences from the shipped platforms

**No `io_uring`.** The feature is Linux only, so RocksDB uses the POSIX I/O
backend. This is the same code path every non-Linux build takes.

**No systemd integration.** Readiness and watchdog notification, socket
activation, and the reload handling described in
[Systemd Socket Activation](socket-activation.md) and
[Reloading Configuration](configuration-reload.md) are all Linux only. Run the
server under `rc.d` or a supervisor of your choice instead.

**No CPU feature levels.** The `-v1` through `-v4` distinction that the x86_64
packages carry has no equivalent here. The build targets the baseline for the
architecture.
