use std::time::Duration;

use axum::extract::State;
use axum_client_ip::InsecureClientIp;
use ruma::api::client::{session::get_login_token, uiaa};
use tuwunel_core::{Err, Error, Result, utils};
use tuwunel_service::uiaa::SESSION_ID_LENGTH;

use super::TOKEN_LENGTH;
use crate::Ruma;

/// # `POST /_matrix/client/v1/login/get_token`
///
/// Allows a logged-in user to get a short-lived token which can be used
/// to log in with the m.login.token flow.
///
/// <https://spec.matrix.org/v1.13/client-server-api/#post_matrixclientv1loginget_token>
#[tracing::instrument(skip_all, fields(%client), name = "login_token")]
pub(crate) async fn login_token_route(
	State(services): State<crate::State>,
	InsecureClientIp(client): InsecureClientIp,
	body: Ruma<get_login_token::v1::Request>,
) -> Result<get_login_token::v1::Response> {
	if !services.server.config.login_via_existing_session {
		return Err!(Request(Forbidden("Login via an existing session is not enabled")));
	}

	// This route SHOULD have UIA
	// TODO: How do we make only UIA sessions that have not been used before valid?
	let (sender_user, sender_device) = body.sender();

	let mut uiaainfo = uiaa::UiaaInfo {
		flows: vec![uiaa::AuthFlow { stages: vec![uiaa::AuthType::Password] }],
		completed: Vec::new(),
		params: Box::default(),
		session: None,
		auth_error: None,
	};

	match &body.auth {
		| Some(auth) => {
			let (worked, uiaainfo) = services
				.uiaa
				.try_auth(sender_user, sender_device, auth, &uiaainfo)
				.await?;

			if !worked {
				return Err(Error::Uiaa(uiaainfo));
			}

			// Success!
		},
		| _ => match body.json_body.as_ref() {
			| Some(json) => {
				uiaainfo.session = Some(utils::random_string(SESSION_ID_LENGTH));
				services
					.uiaa
					.create(sender_user, sender_device, &uiaainfo, json);

				return Err(Error::Uiaa(uiaainfo));
			},
			| _ => {
				return Err!(Request(NotJson("No JSON body was sent when required.")));
			},
		},
	}

	let login_token = utils::random_string(TOKEN_LENGTH);
	let expires_in = services
		.users
		.create_login_token(sender_user, &login_token);

	Ok(get_login_token::v1::Response {
		expires_in: Duration::from_millis(expires_in),
		login_token,
	})
}
