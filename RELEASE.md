# Tuwunel 1.4.2

September 12, 2025

Users running maubot, neochat, or any client or bridge not excluded below should update to this patch as soon as possible to reduce unnecessary resource consumption. (see: Bug Fixes)

### New Features

- Requested by @alaviss an alternative DNS resolver has been implemented for use with appservices and other configured targets intended for local networks. This passthru performs minimal caching and cannot be used for federation. Enable with `dns_passthru_appservices = true` or specifying hosts in `dns_passthru_domains` (#158)

- Contributed by @tototomate123 a nifty experimental feature can disable push notifications when you're active on one device from being sent to others. This can be enabled with `suppress_push_when_active`. Please thank them when your pocket stops vibrating while chatting on your desktop! (#150)

- Thanks to a report by @DetermineAbsurd the `m.federate` field can be defaulted to false when creating a room using the new `federate_created_rooms` config option. (#151)

- At the request of @grinapo verbose logging builds are now bundled with this release. These builds are found with the feature-set `-logging-` which is otherwise similar to `-all-`. This contains more messages at all levels optimized away in other release modes; it comes at some performance penalty.

- JWT tokens can now be used for authentication on any endpoint which supports UIA. For example: an external forgot-password service can send a token to the `client/account/password` endpoint to reset a user's password. This feature was commissioned and made public by an enterprise sponsor.

### Enhancements

- Sliding-sync has been significantly refactored. Performance has massively increased with many bugs and compliance issues also fixed. Please be aware we are tracking an issue related to read-marker behavior in Element X. The 🟢 dot does not unconditionally clear at every touch. Whether this is a feature or a bug, or both, is being investigated for v1.5.

- Hydra backports are now enabled by default. The change should be completely transparent. If you do notice any increased load try to increase the `cache_capacity_modifier` above default.

- Room deletions now also purge synctokens which can be significant to the overall storage consumed by a room. Users who have already deleted rooms please be assured an update planned for v1.5 will deal with cleansing synctokens in general.

- Room version 1 and 2 support took a step forward, possibly working for some rooms but is not yet considered adequately supported and the ticket remains open. (#12)

- Thanks to @AreYouLoco for contributing an updated Kubernetes [Helm Chart](https://github.com/AreYouLoco/tuwunel-helm); link added to docs.

### Bug Fixes

- **Special thanks to @frebib for investigating a bug which triggers the uploading of unnecessary encryption one-time-keys.** Running over ten maubot instances it became obvious after observing increased resources and laggy bot response. This update removes any excess keys for a device. Thanks to @duckbuster for confirming neochat is affected. Clients confirmed unaffected include: Element, Element X, Nheko. Fractal, Cinny, matrix-rust-sdk and matrix-js-sdk clients and bots are probably unaffected. Mautrix-based bridges are probably affected. Users of unaffected clients should still upgrade.

- Thanks @dasha_uwu for refactoring alias resolution logic with fixes to remain compatible with the upcoming element-web release. This was an incredibly valuable contribution which will spare all of us from impending grief; the kind of ahead-of-the-game initiative I don't think a project like this could exist without. (adadafa88f3)

- Room deletions now preserve a small number of records to properly synchronize with local clients and remote servers after the room vanishes. Prior behavior is maintained with a `--force` flag added to the command.

- Thanks @scvalex for once again cleaning up our mess after Nix found the github CI was not running doctests. Thank you for contributing the patch 🙏  (#152).

- Thanks @Tronde for reporting a broken link to the CoC in the mdbook documentation. (#155)

- Specification compliance required the `/joined_rooms` endpoint be restricted to current members rather than including past members. (4b49aaad53a)

- Specification compliance required state events be made visible to prior members of a room where `history_visibility=shared`. (86781522b68)

- The `limit` parameter to the `/context` endpoint is now divided with de facto compatibility (matrix-org/matrix-spec#2202)

- The room avatar in sliding sync is now computed with greater compliance to the specification (3deebeab78f). This builds off earlier work done by @tmayoff in (a340e6786db).

- The canonical alias for a room is considered invalid if the primary alias is missing or removed (7221d466ce8). This is a T&S concern and we encourage reports for any other contexts where this condition should be applied.

- Presence is no longer updated by the private read-receipt or read-marker paths, only public receipts.

### Deprecations

- Hardened Malloc support had to be removed after the build broke. We will gladly add support back upon request or contribution.
