# Tuwunel 1.4.0

September 1, 2025

#### Room Version 12 is now stable.

You can freely create these rooms and join them over the federation. Administrators should take note of the following:
- The default room version is still 11. This is due to Matrix compliance tests not yet providing total coverage to make formal assurances about the new version.
- Hydra-backports are not enabled by default. Administrators of high risk servers can enable `hydra_backports` to continue using their pre-v12 rooms with increased security. This will be enabled by default very shortly.
- The ability to upgrade from existing rooms to version 12 is not yet complete as of this release. This decision was purely economical: we provide `hydra_backports` in the interim.

### New Features

- **Deleting Rooms** is now possible thanks to a generous effort by @dasha_uwu. Admins can use the `!admin rooms delete-room` command to force all local users to leave and erase the room from the database.

- An idea by @korewaChino (#136) taken up by serial contribtor @dasha_uwu now grants admins the power to access rooms using `!admin users force-promote` if at least one user on the server has federation-level access to the room. This feature is a major Trust & Safety enhancement.

- Thanks to an idea by @obioma (#118) with an implementation contributed by @dasha_uwu the admin room can be de-federated by setting `federate_admin_room = false` when first setting up a new Tuwunel server. This option is only available for fresh installs.

### Enhancements

- Based on a help request by @mageslayer (#138) the docs for configuring Caddy have been graciously improved by @itzk0tlin (#139).

- Spaces pageloads have been optimized. This primarily affects large and multi-level spaces such as the Matrix Community.

- Building on the room deletion infrastructure contributed by @dasha_uwu an experimental addon can automatically delete empty rooms after the last local user leaves. This is not enabled by default and highly experimental and will not be considered stable until the next release.

### Bug Fixes

- Thanks to a concise report by @alaviss the `/joined_members` and `/members` endpoints now return consistent profile data for each user. Previously the former returned "global profile" data rather than room membership. (#121)

- After a diagnosis by @gardiol the pushers set by a client are now deleted when the associated device logs out. (#120)

- Sync longpoll loop properly terminates for server shutdown thanks to @dasha_uwu.

- Joining restricted rooms with an invite has been fixed by @dasha_uwu.

- Thanks @obioma for making corrections to documentation.
