# Tuwunel 1.4.1

September 2, 2025

Bridge and Application Service users must update from Tuwunel 1.4.0 to this patch. All other users are encouraged to update at their convenience.

### Bug Fixes

- Special thanks to @alaviss for immediately reporting incorrect results from the `/joined_members` endpoint in the v1.4.0 release. This regression primarily affects Application Services and Bridges; the most popular clients have been verified to not make use of this API.
