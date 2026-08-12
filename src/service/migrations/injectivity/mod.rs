//! Short id injectivity: the one-time scan and repair.
//!
//! Releases before v1.8.3 could mint two short ids for one identity,
//! leaving stale reverse rows in both families, ghost entries in a few
//! compressed states, and auth chains cached from both allocations. The
//! migration measures that residue, repairs what matches the shapes it
//! handles, and marks itself complete like any other.

mod repair;
mod scan;

use tuwunel_core::{Result, result::NotFound};

use self::{
	repair::{heal, repair},
	scan::scan,
};
use crate::Services;

/// Global marker recording the repair ran to completion.
///
/// Refused or unverifiable residue leaves it unwritten, so the next boot
/// scans again.
static MARKER: &[u8] = b"fix_short_injectivity";

/// Scan passes one boot allows before giving up on convergence.
///
/// A heal completes torn writes and rescans to re-measure what they
/// explain, and each pass strictly reduces the classes it heals, so the
/// second pass is the one that repairs. The last pass never heals, which
/// bounds a shape that does not settle and keeps the cache-clearing lane
/// reachable on every boot.
const PASSES: usize = 3;

/// Runs the injectivity scan, heal, and repair behind [`MARKER`].
///
/// The stamp follows the repair's own verdict: only a settled repair
/// writes it. A heal rescans rather than repairing, because the orphan and
/// parent counts a refusal turns on are taken against bitmaps the heal
/// changes.
#[tracing::instrument(level = "debug", skip_all)]
pub(super) async fn fix(services: &Services) -> Result {
	let global = &services.db["global"];

	if !global.get(MARKER).await.is_not_found() {
		return Ok(());
	}

	for pass in 1..=PASSES {
		let residue = scan(services).await?;

		// The last pass repairs rather than heals, so the cache-clearing lane
		// still fires on a boot whose heals never settle.
		if pass < PASSES && heal(services, &residue) {
			continue;
		}

		if repair(services, &residue).await? {
			global.insert(MARKER, []);
		}

		break;
	}

	Ok(())
}

/// Stamps the marker on a fresh database.
///
/// A fresh database never ran the unserialized allocator, so there is no
/// residue to scan for.
pub(super) fn mark_clean(services: &Services) { services.db["global"].insert(MARKER, []); }
