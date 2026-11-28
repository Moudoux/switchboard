//! Signing in to the GUI, with passkeys and, when it's set up, single sign-on (see `oidc`).
//! There are no passwords.
//!
//! The first passkey or single sign-on identity is set up through a one-time link from
//! `switchboard enroll`, which only prints to a terminal. Once signed in, more passkeys can be
//! added, and a single sign-on identity linked, from the GUI.

use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;
use serde_json::{Value, json};
use webauthn_rs::prelude::*;

use super::{ApiError, ApiResult};
use crate::state::AppState;
use crate::util::{now_ms, random_token, sha256_hex};

const COOKIE: &str = "switchboard_session";
const SESSION_DAYS: i64 = 30;
const CEREMONY_TIMEOUT: Duration = Duration::from_secs(300);
pub const ENROLL_MINUTES: i64 = 15;

pub enum Ceremony {
    Login {
        state: PasskeyAuthentication,
        expires: Instant,
    },
    Register {
        state: PasskeyRegistration,
        name: String,
        enroll_hash: Option<String>,
        expires: Instant,
    },
}

impl Ceremony {
    fn expired(&self) -> bool {
        let (Ceremony::Login { expires, .. } | Ceremony::Register { expires, .. }) = self;
        *expires < Instant::now()
    }
}

pub fn webauthn(origin: &str) -> anyhow::Result<Webauthn> {
    let url =
        Url::parse(origin).map_err(|e| anyhow::anyhow!("SWITCHBOARD_ORIGIN {origin}: {e}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("SWITCHBOARD_ORIGIN {origin} has no host"))?
        .to_string();
    Ok(WebauthnBuilder::new(&host, &url)?
        .rp_name("Switchboard")
        .allow_any_port(host == "localhost")
        .build()?)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/status", get(status))
        .route("/login/start", post(login_start))
        .route("/login/finish", post(login_finish))
        .route("/register/start", post(register_start))
        .route("/register/finish", post(register_finish))
        .route("/logout", post(logout))
}

/// Passkey management, behind `require_session`.
pub fn passkey_routes() -> Router<AppState> {
    Router::new()
        .route("/passkeys", get(list_passkeys))
        .route("/passkeys/{id}", delete(delete_passkey))
}

pub async fn require_session(
    State(state): State<AppState>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Response {
    match session_valid(&state, &jar).await {
        Ok(true) => next.run(req).await,
        Ok(false) => {
            ApiError::new(axum::http::StatusCode::UNAUTHORIZED, "sign in first").into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn session_valid(state: &AppState, jar: &CookieJar) -> ApiResult<bool> {
    Ok(session_hash(state, jar).await?.is_some())
}

/// The hash of the signed-in session's token, when the cookie holds a live session.
pub(super) async fn session_hash(state: &AppState, jar: &CookieJar) -> ApiResult<Option<String>> {
    let Some(cookie) = jar.get(COOKIE) else {
        return Ok(None);
    };
    let hash = sha256_hex(cookie.value());
    Ok(session_hash_live(&state.db, &hash).await?.then_some(hash))
}

pub(super) async fn session_hash_live(db: &sqlx::SqlitePool, hash: &str) -> sqlx::Result<bool> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM sessions WHERE token_hash = ? AND expires_at > ?")
            .bind(hash)
            .bind(now_ms())
            .fetch_optional(db)
            .await?;
    Ok(found.is_some())
}

/// Checks an enrolment link's token without using it up, and returns its hash.
pub(super) async fn check_enroll_token(state: &AppState, token: &str) -> ApiResult<String> {
    let hash = sha256_hex(token);
    let valid: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM enroll_tokens WHERE token_hash = ? AND expires_at > ?")
            .bind(&hash)
            .bind(now_ms())
            .fetch_optional(&state.db)
            .await?;
    valid.ok_or_else(|| {
        ApiError::unauthorized(
            "this enrolment link has expired or was used; run `switchboard enroll` again",
        )
    })?;
    Ok(hash)
}

/// Uses up an enrolment link's token; false when it has expired or was used meanwhile.
pub(super) async fn use_enroll_token(
    tx: &mut sqlx::SqliteConnection,
    hash: &str,
) -> sqlx::Result<bool> {
    let used = sqlx::query("DELETE FROM enroll_tokens WHERE token_hash = ? AND expires_at > ?")
        .bind(hash)
        .bind(now_ms())
        .execute(tx)
        .await?
        .rows_affected();
    Ok(used > 0)
}

/// How many ways in would be left: passkeys while passkey sign-in is on, and identities from the
/// configured single sign-on provider. `switchboard enroll` always works besides these.
pub(super) async fn ways_in(
    state: &AppState,
    except_passkey: Option<&str>,
    except_identity: Option<i64>,
) -> ApiResult<i64> {
    let passkeys: i64 = if state.config.passkey_login {
        sqlx::query_scalar("SELECT COUNT(*) FROM passkeys WHERE id IS NOT ?")
            .bind(except_passkey)
            .fetch_one(&state.db)
            .await?
    } else {
        0
    };
    let identities: i64 = match &state.oidc {
        Some(oidc) => {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM oidc_identities WHERE issuer = ? AND id IS NOT ?",
            )
            .bind(oidc.issuer())
            .bind(except_identity)
            .fetch_one(&state.db)
            .await?
        }
        None => 0,
    };
    Ok(passkeys + identities)
}

pub(super) async fn start_session(state: &AppState, jar: CookieJar) -> ApiResult<CookieJar> {
    let token = random_token("");
    let now = now_ms();
    sqlx::query("INSERT INTO sessions (token_hash, created_at, expires_at) VALUES (?, ?, ?)")
        .bind(sha256_hex(&token))
        .bind(now)
        .bind(now + SESSION_DAYS * 86_400_000)
        .execute(&state.db)
        .await?;
    let cookie = Cookie::build((COOKIE, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(state.config.cookie_secure)
        .max_age(time::Duration::days(SESSION_DAYS));
    Ok(jar.add(cookie))
}

async fn status(State(state): State<AppState>, jar: CookieJar) -> ApiResult<Json<Value>> {
    let passkeys: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM passkeys")
        .fetch_one(&state.db)
        .await?;
    let identities: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oidc_identities")
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({
        "signedIn": session_valid(&state, &jar).await?,
        "hasPasskeys": passkeys > 0,
        "hasIdentities": identities > 0,
        "passkeyLogin": state.config.passkey_login,
        "sso": state.oidc.as_ref().map(|o| json!({ "label": o.button_label() })),
    })))
}

async fn load_passkeys(state: &AppState) -> ApiResult<Vec<(String, Passkey)>> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, passkey FROM passkeys")
        .fetch_all(&state.db)
        .await?;
    rows.into_iter()
        .map(|(id, json)| {
            Ok((
                id,
                serde_json::from_str(&json)
                    .map_err(|e| ApiError::internal(format!("stored passkey: {e}")))?,
            ))
        })
        .collect()
}

fn credential_id(id: &CredentialID) -> String {
    match serde_json::to_value(id) {
        Ok(Value::String(s)) => s,
        other => format!("{other:?}"),
    }
}

fn put_ceremony(state: &AppState, ceremony: Ceremony) -> String {
    let id = random_token("");
    let mut all = state.ceremonies.lock().expect("ceremony lock");
    all.retain(|_, c| !c.expired());
    all.insert(id.clone(), ceremony);
    id
}

fn take_ceremony(state: &AppState, id: &str) -> Option<Ceremony> {
    state
        .ceremonies
        .lock()
        .expect("ceremony lock")
        .remove(id)
        .filter(|c| !c.expired())
}

fn webauthn_error(e: WebauthnError) -> ApiError {
    tracing::warn!("passkey check failed: {e}");
    ApiError::bad_request(format!("the passkey wasn't accepted ({e})"))
}

async fn login_start(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    if !state.config.passkey_login {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "signing in with a passkey is turned off here",
        ));
    }
    let passkeys: Vec<Passkey> = load_passkeys(&state)
        .await?
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    if passkeys.is_empty() {
        return Err(ApiError::bad_request(
            "no passkey is registered yet; run `switchboard enroll` on the server",
        ));
    }
    let (options, auth) = state
        .webauthn
        .start_passkey_authentication(&passkeys)
        .map_err(webauthn_error)?;
    let ceremony = put_ceremony(
        &state,
        Ceremony::Login {
            state: auth,
            expires: Instant::now() + CEREMONY_TIMEOUT,
        },
    );
    Ok(Json(json!({ "ceremony": ceremony, "options": options })))
}

#[derive(Deserialize)]
struct LoginFinish {
    ceremony: String,
    credential: PublicKeyCredential,
}

async fn login_finish(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<LoginFinish>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let Some(Ceremony::Login { state: auth, .. }) =
        take_ceremony(&state, &req.ceremony).filter(|_| state.config.passkey_login)
    else {
        return Err(ApiError::bad_request(
            "the sign-in took too long; try again",
        ));
    };
    let result = state
        .webauthn
        .finish_passkey_authentication(&req.credential, &auth)
        .map_err(webauthn_error)?;
    let used = credential_id(result.cred_id());
    for (id, mut passkey) in load_passkeys(&state).await? {
        if id == used {
            passkey.update_credential(&result);
            sqlx::query("UPDATE passkeys SET passkey = ?, last_used_at = ? WHERE id = ?")
                .bind(serde_json::to_string(&passkey).map_err(ApiError::internal)?)
                .bind(now_ms())
                .bind(&id)
                .execute(&state.db)
                .await?;
        }
    }
    let jar = start_session(&state, jar).await?;
    Ok((jar, Json(json!({ "ok": true }))))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterStart {
    name: String,
    /// The token from an enrolment link, when not signed in.
    enroll_token: Option<String>,
}

async fn register_start(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<RegisterStart>,
) -> ApiResult<Json<Value>> {
    let name = req.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(ApiError::bad_request(
            "give the passkey a name, such as the device it's on",
        ));
    }
    let enroll_hash = if session_valid(&state, &jar).await? {
        None
    } else {
        let token = req
            .enroll_token
            .as_deref()
            .ok_or_else(|| ApiError::unauthorized("sign in first"))?;
        Some(check_enroll_token(&state, token).await?)
    };
    let existing: Vec<CredentialID> = load_passkeys(&state)
        .await?
        .iter()
        .map(|(_, p)| p.cred_id().clone())
        .collect();
    let user_id = user_id(&state).await?;
    let (options, reg) = state
        .webauthn
        .start_passkey_registration(user_id, "admin", "Switchboard admin", Some(existing))
        .map_err(webauthn_error)?;
    let ceremony = put_ceremony(
        &state,
        Ceremony::Register {
            state: reg,
            name: name.to_string(),
            enroll_hash,
            expires: Instant::now() + CEREMONY_TIMEOUT,
        },
    );
    Ok(Json(json!({ "ceremony": ceremony, "options": options })))
}

/// The one WebAuthn user all passkeys belong to; made on first use.
async fn user_id(state: &AppState) -> ApiResult<Uuid> {
    if let Some(id) = crate::db::setting(&state.db, "webauthn_user_id").await? {
        return Uuid::parse_str(&id).map_err(ApiError::internal);
    }
    let id = Uuid::new_v4();
    crate::db::set_setting(&state.db, "webauthn_user_id", &id.to_string()).await?;
    Ok(id)
}

#[derive(Deserialize)]
struct RegisterFinish {
    ceremony: String,
    credential: RegisterPublicKeyCredential,
}

async fn register_finish(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<RegisterFinish>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let Some(Ceremony::Register {
        state: reg,
        name,
        enroll_hash,
        ..
    }) = take_ceremony(&state, &req.ceremony)
    else {
        return Err(ApiError::bad_request(
            "registering took too long; try again",
        ));
    };
    let passkey = state
        .webauthn
        .finish_passkey_registration(&req.credential, &reg)
        .map_err(webauthn_error)?;
    let mut tx = state.db.begin().await?;
    if let Some(hash) = &enroll_hash
        && !use_enroll_token(&mut tx, hash).await?
    {
        return Err(ApiError::unauthorized(
            "this enrolment link has expired or was used",
        ));
    }
    sqlx::query("INSERT INTO passkeys (id, name, passkey, created_at) VALUES (?, ?, ?, ?)")
        .bind(credential_id(passkey.cred_id()))
        .bind(&name)
        .bind(serde_json::to_string(&passkey).map_err(ApiError::internal)?)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(name, "passkey registered");
    let jar = if enroll_hash.is_some() {
        start_session(&state, jar).await?
    } else {
        jar
    };
    Ok((jar, Json(json!({ "ok": true }))))
}

async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> ApiResult<(CookieJar, Json<Value>)> {
    if let Some(cookie) = jar.get(COOKIE) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(sha256_hex(cookie.value()))
            .execute(&state.db)
            .await?;
    }
    Ok((
        jar.remove(Cookie::build(COOKIE).path("/")),
        Json(json!({ "ok": true })),
    ))
}

async fn list_passkeys(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let rows: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, name, created_at, last_used_at FROM passkeys ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await?;
    let list: Vec<Value> = rows
        .into_iter()
        .map(|(id, name, created, used)| json!({ "id": id, "name": name, "createdAt": created, "lastUsedAt": used }))
        .collect();
    Ok(Json(json!(list)))
}

async fn delete_passkey(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<Json<Value>> {
    if ways_in(&state, Some(&id), None).await? == 0 {
        return Err(ApiError::bad_request(
            "that's the last way to sign in; add a passkey or link single sign-on before removing it",
        ));
    }
    sqlx::query("DELETE FROM passkeys WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// `switchboard enroll`: a one-time link to register a passkey or link single sign-on. It's a
/// secret, so it only goes to a terminal.
pub async fn enroll_command(db: &sqlx::SqlitePool, origin: &str) -> anyhow::Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "`switchboard enroll` prints a sign-in link, so it only runs in a terminal (docker exec -it ...)"
        );
    }
    let token = random_token("enr_");
    sqlx::query("DELETE FROM enroll_tokens WHERE expires_at < ?")
        .bind(now_ms())
        .execute(db)
        .await?;
    sqlx::query("INSERT INTO enroll_tokens (token_hash, expires_at) VALUES (?, ?)")
        .bind(sha256_hex(&token))
        .bind(now_ms() + ENROLL_MINUTES * 60_000)
        .execute(db)
        .await?;
    println!(
        "Open this link within {ENROLL_MINUTES} minutes to register a passkey or link single sign-on. It works once.\n"
    );
    println!("  {origin}/enroll#{token}\n");
    Ok(())
}
