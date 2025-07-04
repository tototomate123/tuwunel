# Tuwunel 1.2.0

July 3, 2025

Appservices can now be declared in the config file. Thanks to @teidesu for this idea. Each appservice can be configured using a TOML section. For example `[global.appservice.mautrix-telegram]` will create an appservice identified as mautrix-telegram. The configuration is similar to the standard Matrix registration.yaml. The appservice will be inactive when commented out. Thanks to @obioma for helping to get the TOML syntax right.

Optimized builds are now available with this release. Multi-platform docker images are available for these optimized builds for everyone using the special-tags like `:latest`, `:preview` or `:main`. The performance boost will be automatic for most users. The standalone binaries, Deb and RPM packages are also built with optimized variants. **Please note the naming scheme has changed and links may be different.**

### New Features

- Declarative Appservices.

- Optimized packages & Multi-platform docker images.

### Bug Fixes

- Special thanks to @orhtej2 for fixing several bugs with LDAP login related to the admin feature. An `admin_base_dn` issue was fixed in https://github.com/matrix-construct/tuwunel/pull/92 and an admin filter issue in https://github.com/matrix-construct/tuwunel/pull/93.

- We owe a lot of gratitude to the effort of @meovary150 for figuring out that using the same `as_token` for more than one appservice can cause obscure bugs. Configurations are now checked to prevent this.

- Thanks to @SKFE396 for reporting that LZ4 support for RocksDB was missing from the built release packages and images. This has now been added back.

- Thanks to @syobocat for debugging a compile error on FreeBSD where our proc-macros were provided empty values for `std::env::args()`.

- Thanks to @k3-cat and @periodic for identifying compression-related issues for the OCI images which failed to load in Podman.

- Thanks to @dasha_uwu for perhaps the third week in a row now, this time for pointing out that `ldap3` was being included as a default Cargo dependency. It's now been properly isolated under its feature-gate.
