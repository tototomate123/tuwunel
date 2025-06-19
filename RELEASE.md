# Tuwunel 1.1.0

June 19, 2025

All dependencies have been fully upgraded for the first time since the
conduwuit transition. RocksDB is now synchronized to 10.2.1-tuwunel for all
builders. The Nix build itself has now been fully migrated from conduwuit;
special thanks to @wkordalski for making this happen. Thanks to @Askhalion
for opening a NixOS package request which you can [vote for here](https://github.com/NixOS/nixpkgs/issues/415469).
An [Arch package](https://aur.archlinux.org/packages/tuwunel) has also been
created in the AUR courtesy of @drrossum in addition to the transitional
package setup by @Kimiblock which we failed to acknowledge during the first
release. The RPM package now has systemd and proper installation added thanks
to a report by @alythemonk.

ARMv8 builds are now supported and bundled with this release. Thanks to
@zaninime and @clement-escolano for reminding us.

JSON Web Token logins are now supported. This feature was commissioned and
made public by an enterprise sponsor. The type `org.matrix.login.jwt` is now
recognized.

### New Features

- JWT login support.

### Follow-up Features

- aarch64 build and packages.
- NixOS build support. (thanks @wkordalski and @coolGi69)
- Dependency upgrades, including Axum 0.8. (thanks @dasha_uwu)
- RPM package systemd and proper installation scripts.

### Bug Fixes

- Changing passwords for pre-migration users was precluded by an error.
Special thanks to @teidesu for making a superb report about this.
