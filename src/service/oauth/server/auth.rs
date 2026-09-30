use std::time::{Duration, SystemTime};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as b64};
use ruma::OwnedUserId;
use serde::{Deserialize, Serialize};
use tuwunel_core::{Err, Result, err, implement, utils, utils::hash::sha256};
use tuwunel_database::{Cbor, Deserialized};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthRequest {
	pub client_id: String,

	pub redirect_uri: String,

	pub scope: String,

	pub state: Option<String>,

	pub nonce: Option<String>,

	pub code_challenge: Option<String>,

	pub code_challenge_method: Option<String>,

	/// The identity provider ID used to authenticate the user for this
	/// authorization request. Stored so it can be propagated to the device
	/// at token exchange time and used for UIAA SSO provider binding.
	pub idp_id: Option<String>,

	/// Set after a successful local login or registration. Provider selection
	/// and local authentication claim the same request under one lock.
	#[serde(default)]
	pub local_auth_selected: bool,

	pub response_mode: Option<String>,

	pub created_at: SystemTime,

	pub expires_at: SystemTime,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AuthCodeSession {
	pub code: String,

	pub client_id: String,

	pub redirect_uri: String,

	pub scope: String,

	pub state: Option<String>,

	pub nonce: Option<String>,

	pub code_challenge: Option<String>,

	pub code_challenge_method: Option<String>,

	pub user_id: OwnedUserId,

	/// Propagated from the originating AuthRequest; identifies which IdP
	/// authenticated the user so the device can be tagged at token exchange.
	pub idp_id: Option<String>,

	pub created_at: SystemTime,

	pub expires_at: SystemTime,
}

pub const AUTH_REQUEST_LIFETIME: Duration = Duration::from_mins(10);
const AUTH_CODE_LIFETIME: Duration = Duration::from_mins(10);
const AUTH_CODE_LENGTH: usize = 64;

#[implement(super::Server)]
#[must_use]
pub fn create_auth_code(&self, auth_req: &AuthRequest, user_id: OwnedUserId) -> String {
	let now = SystemTime::now();
	let code = utils::random_string(AUTH_CODE_LENGTH);
	let session = AuthCodeSession {
		code: code.clone(),
		client_id: auth_req.client_id.clone(),
		redirect_uri: auth_req.redirect_uri.clone(),
		scope: auth_req.scope.clone(),
		state: auth_req.state.clone(),
		nonce: auth_req.nonce.clone(),
		code_challenge: auth_req.code_challenge.clone(),
		code_challenge_method: auth_req.code_challenge_method.clone(),
		user_id,
		idp_id: auth_req.idp_id.clone(),
		created_at: now,
		expires_at: now.checked_add(AUTH_CODE_LIFETIME).unwrap_or(now),
	};

	self.db
		.oidccode_authsession
		.raw_put(&*code, Cbor(&session));

	code
}

#[implement(super::Server)]
pub fn store_auth_request(&self, req_id: &str, request: &AuthRequest) {
	self.db
		.oidcreqid_authrequest
		.raw_put(req_id, Cbor(request));
}

/// Read an authorization request without consuming it.
///
/// A flow that pauses for a user gesture reads the request to decide what to
/// show, then retires it with `remove_auth_request` once the gesture arrives.
/// An unknown or expired request is a `NotFound`, and an expired one is evicted
/// as it is found, so no caller ever renders against a stale request.
#[implement(super::Server)]
pub async fn peek_auth_request(&self, req_id: &str) -> Result<AuthRequest> {
	let request = self
		.db
		.oidcreqid_authrequest
		.get(req_id)
		.await
		.deserialized()
		.map(|cbor: Cbor<AuthRequest>| cbor.0)
		.map_err(|_| err!(Request(NotFound("Unknown or expired authorization request"))))?;

	if SystemTime::now() > request.expires_at {
		self.remove_auth_request(req_id);

		return Err!(Request(NotFound("Authorization request has expired")));
	}

	Ok(request)
}

/// Bind a native authorization request to one selected upstream provider.
/// Only the first selection succeeds; a password submission can no longer use
/// this request once it is bound.
#[implement(super::Server)]
pub async fn bind_auth_request_to_provider(&self, req_id: &str, provider_id: &str) -> Result {
	let _lock = self.auth_request_locks.lock(req_id).await;
	let mut request = self.peek_auth_request(req_id).await?;
	if request.idp_id.is_some() || request.local_auth_selected {
		return Err!(Request(InvalidParam(
			"Authorization request already selected a sign-in method"
		)));
	}

	request.idp_id = Some(provider_id.to_owned());
	self.store_auth_request(req_id, &request);

	Ok(())
}

/// Claim the local-password branch after credentials have succeeded. A
/// concurrent provider selection can win instead, in which case this login
/// cannot issue a token for the provider-bound request.
#[implement(super::Server)]
pub async fn bind_auth_request_to_local(&self, req_id: &str) -> Result {
	let _lock = self.auth_request_locks.lock(req_id).await;
	let mut request = self.peek_auth_request(req_id).await?;
	if request.idp_id.is_some() || request.local_auth_selected {
		return Err!(Request(Forbidden(
			"Authorization request already selected a sign-in method"
		)));
	}

	request.local_auth_selected = true;
	self.store_auth_request(req_id, &request);

	Ok(())
}

/// Undo a local claim when registration rejects a token before consuming it
/// or creating an account, so the user can correct the form or choose SSO.
#[implement(super::Server)]
pub async fn release_local_auth_request(&self, req_id: &str) -> Result {
	let _lock = self.auth_request_locks.lock(req_id).await;
	let mut request = self.peek_auth_request(req_id).await?;
	if !request.local_auth_selected || request.idp_id.is_some() {
		return Err!(Request(Forbidden("Authorization request is not bound to local sign-in")));
	}

	request.local_auth_selected = false;
	self.store_auth_request(req_id, &request);

	Ok(())
}

/// Claim a pending request exactly once, provided it has not changed since the
/// caller validated it. Sharing the lock with provider selection prevents a
/// password flow from claiming a request that was bound to SSO meanwhile.
#[implement(super::Server)]
pub async fn take_auth_request(
	&self,
	req_id: &str,
	expected: &AuthRequest,
) -> Result<AuthRequest> {
	let _lock = self.auth_request_locks.lock(req_id).await;
	let request = self.peek_auth_request(req_id).await?;
	if &request != expected {
		return Err!(Request(Forbidden("Authorization request changed during sign-in")));
	}
	self.remove_auth_request(req_id);

	Ok(request)
}

/// Retire a pending authorization request.
///
/// The request is single-use, so a flow removes it before minting anything
/// against it. Removing a key that is already gone is a no-op.
#[implement(super::Server)]
pub fn remove_auth_request(&self, req_id: &str) { self.db.oidcreqid_authrequest.remove(req_id); }

#[implement(super::Server)]
pub async fn exchange_auth_code(
	&self,
	code: &str,
	client_id: &str,
	redirect_uri: &str,
	code_verifier: Option<&str>,
	require_pkce: bool,
) -> Result<AuthCodeSession> {
	let session: AuthCodeSession = self
		.db
		.oidccode_authsession
		.get(code)
		.await
		.deserialized::<Cbor<_>>()
		.map(|cbor: Cbor<AuthCodeSession>| cbor.0)
		.map_err(|_| err!(Request(Forbidden("Invalid or expired authorization code"))))?;

	self.db.oidccode_authsession.remove(code);

	if SystemTime::now() > session.expires_at {
		return Err!(Request(Forbidden("Authorization code has expired")));
	}
	if session.client_id != client_id {
		return Err!(Request(Forbidden("client_id mismatch")));
	}
	if session.redirect_uri != redirect_uri {
		return Err!(Request(Forbidden("redirect_uri mismatch")));
	}

	let Some(challenge) = &session.code_challenge else {
		// Reject a challenge-less code when PKCE is required: the knob is
		// reloadable and codes outlive an off->on flip of it.
		if require_pkce {
			return Err!(Request(Forbidden(
				"the authorization request carried no PKCE code_challenge"
			)));
		}

		return Ok(session);
	};

	let Some(verifier) = code_verifier else {
		return Err!(Request(Forbidden("code_verifier required for PKCE")));
	};

	validate_code_verifier(verifier)?;

	let method = session
		.code_challenge_method
		.as_deref()
		.unwrap_or("S256");

	// Only S256 is advertised in discovery metadata; reject plain to avoid
	// downgrade attacks (plain challenge == verifier, trivially intercepted).
	let computed = match method {
		| "S256" => b64.encode(sha256::hash(verifier.as_bytes())),
		| _ => return Err!(Request(InvalidParam("Unsupported code_challenge_method"))),
	};

	if computed != *challenge {
		return Err!(Request(Forbidden("PKCE verification failed")));
	}

	Ok(session)
}

/// Validate code_verifier per RFC 7636 Section 4.1: must be 43-128
/// characters using only unreserved characters [A-Z] / [a-z] / [0-9] /
/// "-" / "." / "_" / "~".
fn validate_code_verifier(verifier: &str) -> Result {
	if !(43..=128).contains(&verifier.len()) {
		return Err!(Request(InvalidParam("code_verifier must be 43-128 characters")));
	}

	if !verifier
		.bytes()
		.all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_' || b == b'~')
	{
		return Err!(Request(InvalidParam("code_verifier contains invalid characters")));
	}

	Ok(())
}
