use futures::TryFutureExt;
use ruma::{OwnedUserId, UserId};
use tuwunel_core::{Err, Result, debug_error, err, utils::hash};
use tuwunel_service::Services;

/// Authenticates the given user by its ID and its password.
///
/// Returns the user ID if successful, and an error otherwise.
#[tracing::instrument(skip_all, fields(%user_id), name = "password")]
pub(super) async fn password_login(
	services: &Services,
	user_id: &UserId,
	lowercased_user_id: &UserId,
	password: &str,
) -> Result<OwnedUserId> {
	let (hash, user_id) = services
		.users
		.password_hash(user_id)
		.map_ok(|hash| (hash, user_id))
		.or_else(|_| {
			services
				.users
				.password_hash(lowercased_user_id)
				.map_ok(|hash| (hash, lowercased_user_id))
		})
		.await?;

	if hash.is_empty() {
		return Err!(Request(UserDeactivated("The user has been deactivated")));
	}

	hash::verify_password(password, &hash)
		.inspect_err(|e| debug_error!("{e}"))
		.map_err(|_| err!(Request(Forbidden("Wrong username or password."))))?;

	Ok(user_id.to_owned())
}
