use futures::StreamExt;
use ruma::{MilliSecondsSinceUnixEpoch, ServerName, thirdparty::Medium};
use tuwunel_core::{Result, debug_warn, err, info, utils::stream::TryIgnore, warn};

use super::local_user_id;
use crate::{Services, threepid::canonicalize_email};

/// Adopts the email addresses a foreign database binds in a column of its own.
///
/// Some databases hold one address per localpart in their own column rather
/// than in the third-party identifier store this server reads, so the binding
/// is invisible here and the address looks unclaimed. Adopting it keeps the
/// address answering for its owner, which is what an account authenticated by
/// an identity provider carries instead of a password.
///
/// Addresses are taken one at a time because two foreign addresses can fold
/// onto one canonical key, and a concurrent pass would clear both through the
/// in-use check before either wrote the reverse row.
pub(super) async fn migrate_email_bindings(services: &Services) -> Result {
	let Some(localpart_email) = services.db.open_cf("localpart_email")? else {
		return Ok(());
	};

	let server_name = services.globals.server_name();
	let bound_at = MilliSecondsSinceUnixEpoch::now();
	let cork = services.db.cork_and_sync();

	let (adopted, skipped) = localpart_email
		.stream()
		.ignore_err()
		.fold((0_usize, 0_usize), async |acc, (localpart, address): (&str, &str)| {
			let adopt = Adopt {
				services,
				server_name,
				bound_at,
				localpart,
				address,
			};

			tally(acc, adopt_one(adopt).await)
		})
		.await;

	drop(cork);

	match skipped {
		| 0 if adopted > 0 => info!(%adopted, "Adopted email bindings from a foreign database"),
		| 0 => (),
		| _ => warn!(
			%adopted,
			%skipped,
			"Adopted email bindings from a foreign database; some addresses were left behind"
		),
	}

	Ok(())
}

/// One foreign row and everything needed to bind it locally.
struct Adopt<'a> {
	services: &'a Services,
	server_name: &'a ServerName,
	bound_at: MilliSecondsSinceUnixEpoch,
	localpart: &'a str,
	address: &'a str,
}

/// Binds one foreign row, reporting whether it produced a binding.
///
/// A `false` return is an address already bound to a different account, which
/// has no representation here: this store admits one owner per canonical
/// address, while the foreign column enforces its own uniqueness over the
/// uncanonicalized form.
async fn adopt_one(
	Adopt {
		services,
		server_name,
		bound_at,
		localpart,
		address,
	}: Adopt<'_>,
) -> Result<bool> {
	let user_id = local_user_id(localpart, server_name)
		.ok_or_else(|| err!(SerdeDe("{localpart:?} is not a usable localpart")))?;

	// An account that did not survive the import has no owner to bind to. A
	// deactivated one still reserves its address, here as on the origin, since
	// neither server unhooks a binding on deactivation.
	if !services.users.exists(&user_id).await || user_id == services.globals.server_user {
		return Ok(false);
	}

	let email_canon = canonicalize_email(address)?;

	if services
		.threepid
		.bound_elsewhere(&user_id, &email_canon)
		.await?
	{
		return Ok(false);
	}

	services
		.threepid
		.put_binding(&user_id, &email_canon, Medium::Email, bound_at, bound_at)
		.await;

	Ok(true)
}

fn tally((adopted, skipped): (usize, usize), result: Result<bool>) -> (usize, usize) {
	match result {
		| Ok(true) => (adopted.saturating_add(1), skipped),
		| Ok(false) => (adopted, skipped.saturating_add(1)),
		| Err(e) => {
			debug_warn!(error = %e, "skipping unusable email binding");
			(adopted, skipped.saturating_add(1))
		},
	}
}
