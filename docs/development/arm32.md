# 32-bit ARM

Tuwunel compiles and runs on 32-bit ARM. This is a working port, not a
supported platform: nothing in the CI matrix builds it, and no release artifact
is published for it. This page records how to reproduce the build and the
things that differ from the architectures we do ship.

Verified against 1.8.3 on `armv7-unknown-linux-gnueabihf`. The resulting server
opens its database and answers the client and federation version endpoints
under emulation.


## Why it is not in the build matrix

Every layer in `docker/bake.hcl` inherits its platform from the `system` target,
which declares `platforms` exactly once. A bake target therefore builds its
whole toolchain and runs its compile *inside* a container of the target
platform, and the workflow matrices exclude any job whose `rust_target`,
`sys_target` and runner architecture disagree. Each architecture is built
natively on a matching runner.

There is no 32-bit ARM runner, so adding a bake target would mean running the
compiler itself under emulation. The recipe below cross-compiles instead,
pinning the build stages to `linux/amd64` and only the runtime stage to
`linux/arm/v7`.


## Toolchain

Use Debian's `armhf` cross toolchain, not a musl one. The rest of this section
explains why, because the musl route looks correct and fails late.

Debian `armhf` is ARMv7-A with VFPv3-D16 throughout, which is also what actual
32-bit ARM deployments run, and its GCC and `libstdc++` agree with each other:

```console
$ arm-linux-gnueabihf-g++ -dM -E -x c++ /dev/null | grep -E '__ARM_ARCH |LOCK_FREE'
#define __ARM_ARCH 7
#define __GCC_ATOMIC_INT_LOCK_FREE 2
```

### The musl cross image cannot link this tree

`messense/rust-musl-cross:armv7-musleabihf`, the image the
[RISC-V](riscv.md) recipe uses for its own architecture, ships a GCC whose
default architecture is ARMv5TE despite the ARMv7 tag. Its configure line sets
the float ABI but never `--with-arch` or `--with-fpu`, so GCC falls back to its
built-in ARM default:

```console
$ armv7-unknown-linux-musleabihf-g++ -dM -E -x c++ /dev/null | grep -E '__ARM_ARCH |LOCK_FREE'
#define __ARM_ARCH 5
#define __GCC_ATOMIC_INT_LOCK_FREE 1
```

Everything in that image's sysroot is built at that baseline, `libstdc++.a`
included. Because `libstdc++-v3/src/c++11/futex.cc` is guarded on
`ATOMIC_INT_LOCK_FREE > 1`, it compiles to an empty object there: `futex.o` is
present in the archive but defines no `__atomic_futex_unsigned_base` symbols at
all. Any C++ compiled at ARMv7, where integer atomics are lock free, takes the
futex path in `<future>` and cannot be linked against it. Three RocksDB
translation units use `std::future`, so the failure is unavoidable, and it
arrives at the final link after everything has compiled.

A three line program reproduces it without any of our code:

```cpp
#include <future>
#include <cstdio>
int main() { std::promise<int> p; auto f = p.get_future(); f.wait(); printf("%d\n", f.get()); }
```

```console
$ armv7-unknown-linux-musleabihf-g++ -O2 -std=c++20 t.cc -o t
$ armv7-unknown-linux-musleabihf-g++ -O2 -std=c++20 -march=armv7-a -mfpu=vfpv3-d16 t.cc -o t
undefined reference to `std::__atomic_futex_unsigned_base::_M_futex_wait_until(...)'
undefined reference to `std::__atomic_futex_unsigned_base::_M_futex_notify_all(unsigned int*)'
```

The same family's `aarch64-musl` image is unaffected, so this is a property of
that ARM32 configuration rather than of musl cross toolchains in general.
Dropping `-mfpu=vfpv3-d16` fails earlier still, with "selected architecture
lacks an FPU", because `--with-fpu` is unset as well.

The practical consequence is that this port is glibc rather than musl, and the
binary is therefore dynamically linked. That is the opposite of what the target
itself suggests: unlike `riscv64gc-unknown-linux-musl`, the musl ARM targets do
default `crt-static` on, so a static build would be the natural outcome if a
correctly configured toolchain were available.


## Building a container image

Save this as `Dockerfile.armv7` and build it from the repository root. The
repository `.dockerignore` already excludes `target/` and `.git`.

```dockerfile
# syntax=docker/dockerfile:1

ARG RUST_TARGET=armv7-unknown-linux-gnueabihf
ARG CROSS=arm-linux-gnueabihf
ARG SYSROOT=/usr/arm-linux-gnueabihf

# The build stages are pinned to linux/amd64 rather than $BUILDPLATFORM so that
# the compiler runs natively on an amd64 host. Elsewhere it runs under
# emulation, which is still far cheaper than emulating the compiler as a 32-bit
# ARM binary.
FROM --platform=linux/amd64 rust:1.95.0-bookworm AS toolchain

ARG RUST_TARGET
ARG CROSS

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        g++-arm-linux-gnueabihf libclang-dev clang cmake; \
    rustup target add "${RUST_TARGET}"

# Assert the properties this toolchain is chosen for rather than trusting the
# package name, since the musl image's tag is exactly what misleads.
RUN set -eux; \
    "${CROSS}-g++" -dM -E -x c++ /dev/null | grep -qx '#define __ARM_ARCH 7'; \
    "${CROSS}-g++" -dM -E -x c++ /dev/null | grep -qx '#define __GCC_ATOMIC_INT_LOCK_FREE 2'; \
    find / -name libstdc++.a -path "*${CROSS}*" -print -quit \
        | xargs "${CROSS}-nm" --defined-only | grep -q _M_futex_wait_until

FROM toolchain AS build

WORKDIR /usr/src/tuwunel
COPY .cargo .cargo
COPY Cargo.toml Cargo.lock clippy.toml rustfmt.toml tuwunel-example.toml ./
COPY src src

ARG RUST_TARGET
ARG CROSS
ARG SYSROOT
ARG FEATURES="brotli_compression,element_hacks,gzip_compression,jemalloc,jemalloc_conf,media_thumbnail,release_max_log_level,systemd,url_preview,zstd_compression"

# Pinning the clang triple and sysroot matters more here than on a 64-bit
# target: if bindgen fell back to the host headers it would generate bindings
# with 64-bit long and size_t for an ILP32 target, which links but is wrong.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/usr/src/tuwunel/target,sharing=locked \
    set -eux; \
    CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER="${CROSS}-gcc" \
    CC_armv7_unknown_linux_gnueabihf="${CROSS}-gcc" \
    CXX_armv7_unknown_linux_gnueabihf="${CROSS}-g++" \
    AR_armv7_unknown_linux_gnueabihf="${CROSS}-ar" \
    LIBCLANG_PATH=/usr/lib/llvm-14/lib \
    ROCKSDB_STATIC=1 \
    BINDGEN_EXTRA_CLANG_ARGS="--target=${CROSS} --sysroot=${SYSROOT}" \
    cargo build --release --target "${RUST_TARGET}" -p tuwunel \
        --no-default-features --features "${FEATURES}"; \
    install -Dm755 "target/${RUST_TARGET}/release/tuwunel" /out/tuwunel

FROM --platform=linux/arm/v7 debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /out/tuwunel /usr/local/bin/tuwunel

RUN ["/usr/local/bin/tuwunel", "--version"]

ENV TUWUNEL_DATABASE_PATH=/var/lib/tuwunel
VOLUME /var/lib/tuwunel
EXPOSE 8008
ENTRYPOINT ["/usr/local/bin/tuwunel"]
```

```bash
docker buildx build --platform linux/arm/v7 \
    -f Dockerfile.armv7 -t tuwunel:armv7 --load .
```

A cold build takes about an hour on an ARM64 host, where the build stages
themselves run under amd64 emulation, and produces a 216 MB image holding a
74 MB binary. The `RUN ["/usr/local/bin/tuwunel", "--version"]` line executes
under emulation during the build, so a binary that cannot run fails the build
rather than shipping.

The runtime stage uses Debian rather than Alpine because the binary is linked
against glibc.


## Running under emulation

No extra tooling is required if the host has `binfmt_misc` handlers registered
for 32-bit ARM, which Docker Desktop does by default. Note that `docker buildx
inspect` under-reports here and may omit `linux/arm/v7` from its platform list
even when it works, so confirm by running something instead:

```bash
docker run --rm --platform linux/arm/v7 debian:bookworm-slim uname -m
```

That prints `armv7l`. Then run the image as usual:

```bash
docker run --rm --platform linux/arm/v7 -p 8008:8008 tuwunel:armv7 \
    -O server_name='"example.com"' -O address='"0.0.0.0"' -O port=8008
```

Startup is unhurried but not slow: the database opens its 133 column families
in about two and a quarter seconds, and the server answers requests within
about nine seconds of launch.


## Differences from the shipped architectures

**The address space is the operating limit.** A 32-bit process has a user
address space of a few gigabytes no matter how much memory the machine has, and
that ceiling, not the installed RAM, is what bounds the caches. Several defaults
scale with the host's parallelism rather than its memory, so a board with many
cores and little address space is the awkward case:
`db_cache_capacity_mb`, `db_write_buffer_capacity_mb`, `pdu_cache_capacity` and
`auth_chain_cache_capacity` all grow with the core count, and
`cache_capacity_modifier` scales the caches as a group. Expect to set these
explicitly rather than relying on the defaults.

**The binary is dynamically linked.** It needs glibc plus the GCC runtime
libraries, which is a consequence of the toolchain situation described above
rather than a property of the target:

```console
$ ldd /usr/local/bin/tuwunel
        libstdc++.so.6 => /lib/arm-linux-gnueabihf/libstdc++.so.6
        libgcc_s.so.1 => /lib/arm-linux-gnueabihf/libgcc_s.so.1
        libm.so.6 => /lib/arm-linux-gnueabihf/libm.so.6
        libc.so.6 => /lib/arm-linux-gnueabihf/libc.so.6
        /lib/ld-linux-armhf.so.3
```

**`io_uring` is off here.** The recipe omits it, so a 32-bit ARM image built
this way is not feature equivalent to an x86_64 or ARM64 one.

**No CPU feature levels.** The `sys_target` dimension has no 32-bit ARM
equivalent here. The build targets the Debian `armhf` baseline, ARMv7-A with
VFPv3-D16, and selects no profile above it.

**ARMv6 is untested.** Rust defines targets for it, and the Raspberry Pi 1 and
Zero need them, but nothing here has been built or run against one. Expect the
libstdc++ trap above to matter more rather than less at that baseline.


## Adding it to the build matrix

Should CI ever gain a 32-bit ARM runner, the changes to `docker/bake.hcl` follow
the shape the ARM64 entries already have: an `armv7-v7-linux-gnu` value for
`sys_target`, a `["linux/arm/v7"]` branch in the `platforms` expression on the
`system` target, and `-ftls-model=initial-exec` in `rocksdb_cxx_flags` without
ARM64's `-mno-outline-atomics` or x86_64's `-mpclmul`. The `-l:libgcc.a` entry
ARM64 carries is needed here too, since 32-bit ARM resolves helper routines
through libgcc that the hardware has no instruction for, `__aeabi_uldivmod` and
the rest of the integer division family among them.

The base layers would also need every apt package they install to exist for
`armhf`, and the address space limits above make a 32-bit runner a poor host for
the test suite even once it builds.
