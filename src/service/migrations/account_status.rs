use std::sync::Arc;

use futures::StreamExt;
use ruma::{OwnedUserId, UserId};
use tuwunel_core::{
	Result, info,
	utils::{
		ReadyExt,
		option::OptionExt,
		stream::{BroadbandExt, TryIgnore},
	},
};
use tuwunel_database::Map;

use super::local_user_id;
use crate::{
	Services,
	users::{PASSWORD_DISABLED, PASSWORD_SENTINEL},
};

/// Reconciles account states a foreign database keeps outside the password
/// column.
///
/// Some databases mark deactivation in a column of its own while leaving the
/// hash in place, and spell an account authenticated elsewhere as an empty
/// hash. This server reads both states from the password alone, so until they
/// are adopted a deactivated account reads as active and an externally
/// authenticated one reads as deactivated. Adoption happens once, because
/// local administration writes the same column afterward and a second pass
/// would undo it.
pub(super) async fn migrate_account_status(services: &Services) -> Result {
	let deactivated = services.db.open_cf("userid_deactivated")?;
	let subjects = services.db.open_cf("openidsubject_localpart")?;

	if let Some(deactivated) = deactivated.as_ref() {
		adopt_deactivations(services, deactivated).await;
	}

	if let Some(subjects) = subjects.as_ref() {
		adopt_passwordless(services, subjects, deactivated.as_ref()).await;
	}

	Ok(())
}

/// Empties the password of every account a foreign column marks deactivated.
///
/// The marker is invisible here while the surviving hash reads as an active
/// account, restoring a login the origin had already withdrawn. An empty
/// password is the same state spelled locally, and costs the foreign hash,
/// which decides nothing for an account deactivated on both sides.
async fn adopt_deactivations(services: &Services, deactivated: &Arc<Map>) {
	let userid_password = &services.db["userid_password"];
	let cork = services.db.cork_and_sync();

	let adopted = deactivated
		.keys::<&UserId>()
		.ignore_err()
		.map(ToOwned::to_owned)
		.broad_filter_map(async |user_id: OwnedUserId| {
			services
				.users
				.is_active(&user_id)
				.await
				.then_some(user_id)
		})
		.ready_fold(0_usize, |adopted, user_id| {
			userid_password.insert(&user_id, PASSWORD_DISABLED);

			adopted.saturating_add(1)
		})
		.await;

	drop(cork);

	if adopted > 0 {
		info!(%adopted, "Adopted deactivated accounts from a foreign database");
	}
}

/// Restores the sentinel password on accounts an identity provider
/// authenticates.
///
/// A foreign database spells "no local password" as an empty hash, which reads
/// here as deactivated and refuses the account every login flow. Only accounts
/// carrying a provider subject are restored, because an empty hash on its own
/// cannot be told apart from a deactivation this server wrote.
///
/// The sentinel carries that meaning locally, leaving the account active with
/// no password to verify against, while an account the foreign column marks
/// deactivated keeps its deactivation.
async fn adopt_passwordless(
	services: &Services,
	subjects: &Arc<Map>,
	deactivated: Option<&Arc<Map>>,
) {
	let userid_password = &services.db["userid_password"];
	let server_name = services.globals.server_name();
	let cork = services.db.cork_and_sync();

	let adopted = subjects
		.stream()
		.ignore_err()
		.ready_filter_map(|(_, localpart): (&str, &str)| local_user_id(localpart, server_name))
		.broad_filter_map(async |user_id: OwnedUserId| {
			restorable(services, deactivated, &user_id)
				.await
				.then_some(user_id)
		})
		.ready_fold(0_usize, |adopted, user_id| {
			userid_password.insert(&user_id, PASSWORD_SENTINEL);

			adopted.saturating_add(1)
		})
		.await;

	drop(cork);

	if adopted > 0 {
		info!(%adopted, "Restored accounts authenticated elsewhere from a foreign database");
	}
}

/// Reports whether the account reads as deactivated here without the foreign
/// column marking it so.
///
/// The empty password a foreign database gives an account authenticated
/// elsewhere is the byte pattern this server writes for a deactivation, so the
/// foreign marker is the only thing separating them.
async fn restorable(
	services: &Services,
	deactivated: Option<&Arc<Map>>,
	user_id: &UserId,
) -> bool {
	let passwordless = services
		.users
		.is_deactivated(user_id)
		.await
		.unwrap_or(false);

	passwordless
		&& deactivated
			.map_async(|deactivated| deactivated.exists(user_id))
			.await
			.is_none_or(|marked| marked.is_err())
}
