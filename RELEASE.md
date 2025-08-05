# Tuwunel 1.3.0

August 4, 2025

Preparations for [Project Hydra](https://matrix.org/blog/2025/07/security-predisclosure/) have taken place. Users should be prepared to upgrade to `v1.4.0-rc` next week. Due to the comprehensive low-level changes which have taken place, and the inability to develop them in public with your feedback, the release will undergo an extended `-rc` period. Due to the time difference and scheduling conflicts our release may be published between 12 to 36 hours after the suggested time; though well before the written disclosure of the issues.

Some foundational work for `v1.4.0` was backported into this release after the announcement was made mid-July to further extend Hydra. An unexpected performance improvement drastically reduced CPU usage. As a result, integration tests began to flake, uncovering latent concurrency bugs which were addressed. These fixes primarily impact the legacy and sliding-sync systems, and further fixes improve performance and compliance, though mostly in the legacy system as sliding-sync lacks adequate test coverage.

This release fixes many bugs and improves performance but at the cost of planned features further rescheduled to either `v1.4.1` or `v1.5.0`.


### New Features

- Refresh tokens (MSC2918) have been implemented. Supporting clients can now timeout their access tokens with a soft-logout after a configured `access_token_ttl`. This feature was commissioned and made public by an enterprise sponsor.

- Typing indicators have been added to sliding-sync thanks to @tmayoff. This feature (and the whole of sliding-sync) is still experimental and the indicator may not always appear or disappear as intended, nevertheless the effort will be enhanced by foundational fixes improving sliding-sync requested soon by the project's sponsor.


### Enhancements

- @dasha_uwu maintains their streak as a serial contributor by patching the `!admin query raw` command with a base64 option allowing for low-level debugging of database records.

- @obioma has improved the documentation explaining how to use multiple configuration files with precedence.

- Upon recommendation of @grin a basic request ID has been added to the tracing logs to uniquely distinguish each request while it's interleaved among others.

- Requested by @fruzitent this and future releases are tagged by version as multi-arch docker images to be properly archived in the registry rather than simply overwriting `:latest`.

- Event processing performance has been improved by fetching and processing `prev_events` and `auth_events` concurrently. This reduces the impact of recursing large graphs while the room's mutex is locked.

- An experimental command `!admin debug resync-database` has been added for developers curious about #35


### Bug Fixes

- Thanks to @tmayoff room avatars are properly calculated and no longer the same for all spaces (https://github.com/matrix-construct/tuwunel/pull/102).

- Courtesy of @coolGi69 our bump to Rust 1.88 was properly updated for Nix. Apologies to the Nix community for getting this wrong the first time.

- Invite rejections have been fixed, this was due to a misinterpretation of the spec in legacy sync.

- Knock rooms might have been buggy from the ambiguous overuse of the word "count" in the codebase. Database records which expected a summation of the users in a room instead received the sequence number of the server, both are called "count."

- Room knocks failed to wakeup the sync systems; some cases of account_data changes also failed to wakeup the sync systems. These have been addressed.

- The main sequence number fundamental to the entire server's operation (the "count" or counter) has been refactored after having exceeded architectural limitations. It has been replaced by a two-phase counter ensuring read-after-write consistency, and quasi-transactions grouping multiple writes.

- Sequence issues have been addressed in both legacy and sliding sync. These systems operate using a "snapshot" approach which intentionally ignores new data received by the server after the sync request has started; the snapshot approach replaced the complex of mutexes used by Conduit. The server was not originally designed for this approach and some information "from the future" continued to leak into the snapshot's window; these leaks have been sealed.

- Protocol compliance issues in legacy sync have been addressed. Additional compliance tests for device list updates now pass. The `state` and `timeline` on incremental sync provide expected results in more (if not all) cases.

- Errors requiring M_BAD_ALIAS instead of M_UNKNOWN when sending `m.room.canonical_alias` are now conforming.


### Deprecations

- Unauthenticated media fallbacks are no longer requested by default. This can still be enabled with `request_legacy_media` if desired.

- Legacy Sliding-Sync has been removed in favor of Simplified Sliding-Sync. Clients which exclusively using Sliding-Sync have already migrated around the start of this year, so this removal should have no impact now.
