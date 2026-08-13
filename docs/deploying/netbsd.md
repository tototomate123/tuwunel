# Tuwunel for NetBSD

Tuwunel builds and runs on NetBSD, but NetBSD is not a supported platform.
Nothing in the CI matrix builds it, no release artifact is published for it, and
there is no pkgsrc package. This page records a build that was verified end to
end, and the handful of things that differ from the platforms we do ship.

Verified against 1.8.3 on NetBSD 11.0, `evbarm-aarch64`. The resulting server
opens its database, answers the client and federation version endpoints,
registers an account, creates a room, sends and reads back a message, and exits
cleanly on `SIGTERM`.

Contributions for getting Tuwunel into pkgsrc are welcome.


## Toolchain

A stock NetBSD install has `pkg_add` but no `pkgin`, so bootstrap it first:

```sh
export PKG_PATH=https://cdn.NetBSD.org/pub/pkgsrc/packages/NetBSD/aarch64/11.0/All/
pkg_add pkgin
echo "https://cdn.NetBSD.org/pub/pkgsrc/packages/NetBSD/aarch64/11.0/All" \
    > /usr/pkg/etc/pkgin/repositories.conf
pkgin update
```

Then install the build dependencies:

```sh
pkgin install rust git cmake clang gmake pkgconf curl
```

`rust` carries 1.96.0 on this branch, which is at or above the `rust-version`
the workspace declares. The `clang` package supplies `/usr/pkg/lib/libclang.so`,
which `rust-librocksdb-sys` runs bindgen against; point the build at it with
`LIBCLANG_PATH`. NetBSD's base compiler is GCC and does not provide it.

Everything under `/usr/pkg` needs to be on `PATH`, which is not the case for a
non-interactive `ssh` command:

```sh
export PATH=/usr/pkg/bin:/usr/pkg/sbin:$PATH
```

This is the set that was verified rather than a minimal one. `curl` is only used
by the checks further down this page.


## Features

The default feature set includes `io_uring` and `systemd`, both of which are
Linux only. Every NetBSD build therefore has to opt out of the defaults and name
its features explicitly:

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


## Building

```sh
export PATH=/usr/pkg/bin:/usr/pkg/sbin:$PATH
export LIBCLANG_PATH=/usr/pkg/lib

cargo build --release -p tuwunel --no-default-features \
    --features brotli_compression,element_hacks,gzip_compression,jemalloc,jemalloc_conf,media_thumbnail,release_max_log_level,url_preview,zstd_compression
```

No source changes or compiler flag workarounds are needed. Unlike FreeBSD on the
same architecture, RocksDB's `crc32c_arm64.cc` compiles cleanly here, because
none of its platform specific branches apply to NetBSD.

A cold build took about 66 minutes on 4 jobs and produced a 79 MB binary. The
result is dynamically linked, and against base system libraries only:

```console
$ ldd target/release/tuwunel
	-lstdc++.9 => /usr/lib/libstdc++.so.9
	-lm.0 => /usr/lib/libm.so.0
	-lgcc_s.1 => /usr/lib/libgcc_s.so.1
	-lc.12 => /usr/lib/libc.so.12
	-lpthread.1 => /usr/lib/libpthread.so.1
```

Nothing under `/usr/pkg` is needed at run time, so the binary can be copied to a
host that has no pkgsrc toolchain installed.


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
tuwunel -c /usr/pkg/etc/tuwunel/tuwunel.toml
```

### The jemalloc notice

Every invocation, `--version` included, prints one line to standard error:

```
<jemalloc>: No getcpu support: percpu_arena:percpu
```

Tuwunel compiles a `malloc_conf` string into the binary that asks for
`percpu_arena:percpu`, which needs a way to ask which CPU the calling thread is
running on. NetBSD does not offer one, so jemalloc falls back to its normal
arena assignment and says so. Nothing else changes, and the notice is cosmetic.
Building without the `jemalloc_conf` feature silences it, at the cost of the
rest of the tuned allocator configuration.


## Running under rc.d

Create an account for the service, install the binary, and give it a database
directory:

```sh
groupadd tuwunel
useradd -g tuwunel -d /nonexistent -s /sbin/nologin tuwunel
install -m 755 target/release/tuwunel /usr/pkg/bin/tuwunel
mkdir -p /usr/pkg/etc/tuwunel /var/db/tuwunel
chown tuwunel:tuwunel /var/db/tuwunel
install -o tuwunel -g tuwunel -m 640 /dev/null /var/log/tuwunel.log
```

That last line matters. NetBSD has no `daemon(8)`, so the script below
backgrounds the server itself and redirects its output, and `rc.subr` applies
the redirect *after* it has dropped to `tuwunel_user`. Without a log file the
account already owns, the start fails with `sh: cannot create
/var/log/tuwunel.log: permission denied`.

Save this as `/etc/rc.d/tuwunel` and `chmod 555` it:

```sh
#!/bin/sh
#
# PROVIDE: tuwunel
# REQUIRE: DAEMON NETWORKING
# KEYWORD: shutdown

$_rc_subr_loaded . /etc/rc.subr

name="tuwunel"
rcvar=$name
command="/usr/pkg/bin/tuwunel"
command_args="-c /usr/pkg/etc/tuwunel/tuwunel.toml >> /var/log/tuwunel.log 2>&1 &"
tuwunel_user="tuwunel"

load_rc_config $name
run_rc_command "$1"
```

```sh
echo "tuwunel=YES" >> /etc/rc.conf
/etc/rc.d/tuwunel start
```

Set `log_colors = false` in the configuration when logging to a file this way.
Tuwunel decides on colour from the configuration rather than from whether the
sink is a terminal, so the log otherwise accumulates ANSI escapes.


## Differences from the shipped platforms

**CRC32C runs in software on arm64.** RocksDB decides at run time whether the
CPU has the CRC32 and PMULL extensions. `crc32c_arm64.cc` can answer that
question through `getauxval` on Linux, `elf_aux_info` on FreeBSD, `sysctlbyname`
on Apple platforms, and `sysctl` on OpenBSD. NetBSD matches none of those, so
the check falls through to `return 0` and the accelerated paths are never
selected, even on hardware that supports them. Checksumming is on the hot path
for every read and write, so expect it to cost throughput relative to a platform
where detection works. Teaching that file to query NetBSD would be a worthwhile
contribution upstream.

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
