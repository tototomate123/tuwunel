mod ldap;
mod logout;
mod password;
mod token;

use axum::extract::State;
use axum_client_ip::InsecureClientIp;
use futures::FutureExt;
use ruma::{
	UserId,
	api::client::{
		session::{
			get_login_types::{
				self,
				v3::{ApplicationServiceLoginType, PasswordLoginType, TokenLoginType},
			},
			login::{
				self,
				v3::{DiscoveryInfo, HomeserverInfo},
			},
		},
		uiaa,
	},
};
use tuwunel_core::{Err, Result, debug, err, info, utils, utils::stream::ReadyExt};

use self::{ldap::ldap_login, password::password_login};
pub(crate) use self::{
	logout::{logout_all_route, logout_route},
	token::login_token_route,
};
use super::{DEVICE_ID_LENGTH, TOKEN_LENGTH};
use crate::Ruma;

/// # `GET /_matrix/client/v3/login`
///
/// Get the supported login types of this server. One of these should be used as
/// the `type` field when logging in.
#[tracing::instrument(skip_all, fields(%client), name = "login")]
pub(crate) async fn get_login_types_route(
	State(services): State<crate::State>,
	InsecureClientIp(client): InsecureClientIp,
	_body: Ruma<get_login_types::v3::Request>,
) -> Result<get_login_types::v3::Response> {
	Ok(get_login_types::v3::Response::new(vec![
		get_login_types::v3::LoginType::Password(PasswordLoginType::default()),
		get_login_types::v3::LoginType::ApplicationService(ApplicationServiceLoginType::default()),
		get_login_types::v3::LoginType::Token(TokenLoginType {
			get_login_token: services.server.config.login_via_existing_session,
		}),
	]))
}

/// # `POST /_matrix/client/v3/login`
///
/// Authenticates the user and returns an access token it can use in subsequent
/// requests.
///
/// - The user needs to authenticate using their password (or if enabled using a
///   json web token)
/// - If `device_id` is known: invalidates old access token of that device
/// - If `device_id` is unknown: creates a new device
/// - Returns access token that is associated with the user and device
///
/// Note: You can use [`GET
/// /_matrix/client/r0/login`](fn.get_supported_versions_route.html) to see
/// supported login types.
#[tracing::instrument(skip_all, fields(%client), name = "login")]
pub(crate) async fn login_route(
	State(services): State<crate::State>,
	InsecureClientIp(client): InsecureClientIp,
	body: Ruma<login::v3::Request>,
) -> Result<login::v3::Response> {
	let emergency_mode_enabled = services.config.emergency_password.is_some();

	// Validate login method
	// TODO: Other login methods
	let user_id = match &body.login_info {
		#[allow(deprecated)]
		| login::v3::LoginInfo::Password(login::v3::Password {
			identifier,
			password,
			user,
			..
		}) => {
			debug!("Got password login type");
			let user_id =
				if let Some(uiaa::UserIdentifier::UserIdOrLocalpart(user_id)) = identifier {
					UserId::parse_with_server_name(user_id, &services.config.server_name)
				} else if let Some(user) = user {
					UserId::parse_with_server_name(user, &services.config.server_name)
				} else {
					return Err!(Request(Unknown(
						debug_warn!(?body.login_info, "Valid identifier or username was not provided (invalid or unsupported login type?)")
					)));
				}
				.map_err(|e| err!(Request(InvalidUsername(warn!("Username is invalid: {e}")))))?;

			let lowercased_user_id = UserId::parse_with_server_name(
				user_id.localpart().to_lowercase(),
				&services.config.server_name,
			)?;

			if !services.globals.user_is_local(&user_id)
				|| !services
					.globals
					.user_is_local(&lowercased_user_id)
			{
				return Err!(Request(Unknown("User ID does not belong to this homeserver")));
			}

			if cfg!(feature = "ldap") && services.config.ldap.enable {
				ldap_login(&services, &user_id, &lowercased_user_id, password)
					.boxed()
					.await?
			} else {
				password_login(&services, &user_id, &lowercased_user_id, password)
					.boxed()
					.await?
			}
		},
		| login::v3::LoginInfo::Token(login::v3::Token { token }) => {
			debug!("Got token login type");
			if !services.server.config.login_via_existing_session {
				return Err!(Request(Unknown("Token login is not enabled.")));
			}
			services
				.users
				.find_from_login_token(token)
				.await?
		},
		#[allow(deprecated)]
		| login::v3::LoginInfo::ApplicationService(login::v3::ApplicationService {
			identifier,
			user,
		}) => {
			debug!("Got appservice login type");

			let Some(ref info) = body.appservice_info else {
				return Err!(Request(MissingToken("Missing appservice token.")));
			};

			let user_id =
				if let Some(uiaa::UserIdentifier::UserIdOrLocalpart(user_id)) = identifier {
					UserId::parse_with_server_name(user_id, &services.config.server_name)
				} else if let Some(user) = user {
					UserId::parse_with_server_name(user, &services.config.server_name)
				} else {
					return Err!(Request(Unknown(
						debug_warn!(?body.login_info, "Valid identifier or username was not provided (invalid or unsupported login type?)")
					)));
				}
				.map_err(|e| err!(Request(InvalidUsername(warn!("Username is invalid: {e}")))))?;

			if !services.globals.user_is_local(&user_id) {
				return Err!(Request(Unknown("User ID does not belong to this homeserver")));
			}

			if !info.is_user_match(&user_id) && !emergency_mode_enabled {
				return Err!(Request(Exclusive("Username is not in an appservice namespace.")));
			}

			user_id
		},
		| _ => {
			debug!("/login json_body: {:?}", &body.json_body);
			return Err!(Request(Unknown(
				debug_warn!(?body.login_info, "Invalid or unsupported login type")
			)));
		},
	};

	// Generate new device id if the user didn't specify one
	let device_id = body
		.device_id
		.clone()
		.unwrap_or_else(|| utils::random_string(DEVICE_ID_LENGTH).into());

	// Generate a new token for the device
	let token = utils::random_string(TOKEN_LENGTH);

	// Determine if device_id was provided and exists in the db for this user
	let device_exists = if body.device_id.is_some() {
		services
			.users
			.all_device_ids(&user_id)
			.ready_any(|v| v == device_id)
			.await
	} else {
		false
	};

	if device_exists {
		services
			.users
			.set_token(&user_id, &device_id, &token)
			.await?;
	} else {
		services
			.users
			.create_device(
				&user_id,
				&device_id,
				&token,
				body.initial_device_display_name.clone(),
				Some(client.to_string()),
			)
			.await?;
	}

	// send client well-known if specified so the client knows to reconfigure itself
	let client_discovery_info: Option<DiscoveryInfo> = services
		.server
		.config
		.well_known
		.client
		.as_ref()
		.map(|server| DiscoveryInfo::new(HomeserverInfo::new(server.to_string())));

	info!("{user_id} logged in");

	#[allow(deprecated)]
	Ok(login::v3::Response {
		user_id,
		access_token: token,
		device_id,
		well_known: client_discovery_info,
		expires_in: None,
		home_server: Some(services.config.server_name.clone()),
		refresh_token: None,
	})
}
