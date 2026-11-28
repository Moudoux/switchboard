//! Single sign-on through an OpenID Connect provider: the authorization code flow with PKCE (S256),
//! `state` and `nonce`, as a confidential client. Endpoints come from the issuer's discovery
//! document, and the ID token is checked by the `openidconnect` crate: its signature against the
//! provider's JWKS, `iss`, `aud`, `exp` and `nonce`.
//!
//! An identity is the issuer and its `sub`, never the email address, and it can only sign in once
//! it's linked: from Settings while signed in, or through a `switchboard enroll` link.
//!
//! What a sign-in needs to carry over the provider's redirect (the state, the nonce, the PKCE
//! verifier and what the sign-in is for) is kept here, in memory, under the hash of a random
//! token. The token goes in a short-lived `SameSite=Lax` cookie, which the browser still sends on
//! the top-level redirect back from the provider. The session cookie is `SameSite=Strict` and so is
//! not sent there, which is why a link made from Settings remembers the session it started from.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use openidconnect::core::{CoreClient, CoreProviderMetadata, CoreResponseType};
use openidconnect::{
    AccessTokenHash, AsyncHttpClient, AuthenticationFlow, AuthorizationCode, ClientId, CsrfToken,
    EndpointMaybeSet, EndpointNotSet, EndpointSet, HttpRequest, HttpResponse, IssuerUrl, Nonce,
    OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ApiError, ApiResult, auth};
use crate::config::OidcConfig;
use crate::state::AppState;
use crate::util::{now_ms, random_token, sha256_hex};

const FLOW_COOKIE: &str = "switchboard_oidc";
const FLOW_COOKIE_PATH: &str = "/api/auth/oidc";
pub const CALLBACK_PATH: &str = "/api/auth/oidc/callback";
const FLOW_TIMEOUT: Duration = Duration::from_secs(600);
/// Sign-ins in progress at once; starting one needs no session, so this bounds the memory used.
const MAX_FLOWS: usize = 1000;
/// How long the discovery document and the provider's keys are used before fetching them again.
const METADATA_TTL: Duration = Duration::from_secs(600);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

type Client = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// The configured provider, its cached discovery document, and the sign-ins in progress.
pub struct Oidc {
    config: OidcConfig,
    issuer: IssuerUrl,
    redirect: RedirectUrl,
    http: Http,
    metadata: tokio::sync::Mutex<Option<(Instant, Arc<CoreProviderMetadata>)>>,
    flows: Mutex<HashMap<String, Flow>>,
}

/// What a sign-in is for, decided when it starts.
#[derive(Debug, Clone, PartialEq)]
enum Purpose {
    SignIn,
    /// Linking from Settings, by the session with this token hash.
    Link {
        session_hash: String,
    },
    /// Linking through an enrolment link, with this token hash.
    Enroll {
        enroll_hash: String,
    },
}

struct Flow {
    state: String,
    nonce: Nonce,
    pkce_verifier: String,
    purpose: Purpose,
    expires: Instant,
}

/// A checked identity from the provider.
#[derive(Debug)]
struct Identity {
    issuer: String,
    subject: String,
    label: String,
}

/// Why a callback didn't sign in, as the code the GUI shows a message for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Failure {
    /// No sign-in in progress for this browser, or it took too long.
    Expired,
    /// The provider answered with an error, such as the user cancelling.
    Denied,
    /// The answer didn't check out: wrong state, bad token, wrong nonce, unreachable provider.
    Failed,
    /// A valid identity that isn't linked to Switchboard.
    Unlinked,
    /// Linking from Settings, but the session ended meanwhile.
    SignedOut,
}

impl Failure {
    fn code(self) -> &'static str {
        match self {
            Failure::Expired => "expired",
            Failure::Denied => "denied",
            Failure::Failed => "failed",
            Failure::Unlinked => "unlinked",
            Failure::SignedOut => "signed-out",
        }
    }
}

impl Oidc {
    pub fn new(config: OidcConfig, origin: &str) -> anyhow::Result<Self> {
        let issuer = IssuerUrl::new(config.issuer.clone())
            .map_err(|e| anyhow::anyhow!("SWITCHBOARD_OIDC_ISSUER {}: {e}", config.issuer))?;
        let redirect = RedirectUrl::new(format!("{origin}{CALLBACK_PATH}"))?;
        // Its own client, apart from the one deliveries use: it trusts the extra CA, and it never
        // follows redirects, as the openidconnect crate asks, so a token can't be sent elsewhere.
        let mut builder = reqwest::Client::builder()
            .user_agent(concat!("switchboard/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15));
        if let Some(path) = &config.ca_file {
            let pem = std::fs::read(path)
                .map_err(|e| anyhow::anyhow!("SWITCHBOARD_OIDC_CA_FILE {}: {e}", path.display()))?;
            let certs = reqwest::Certificate::from_pem_bundle(&pem)
                .map_err(|e| anyhow::anyhow!("SWITCHBOARD_OIDC_CA_FILE {}: {e}", path.display()))?;
            anyhow::ensure!(
                !certs.is_empty(),
                "SWITCHBOARD_OIDC_CA_FILE {} has no certificates",
                path.display()
            );
            builder = builder.tls_certs_merge(certs);
        }
        Ok(Self {
            config,
            issuer,
            redirect,
            http: Http(builder.build()?),
            metadata: tokio::sync::Mutex::new(None),
            flows: Mutex::new(HashMap::new()),
        })
    }

    pub fn issuer(&self) -> &str {
        self.issuer.as_str()
    }

    pub fn button_label(&self) -> &str {
        &self.config.button_label
    }

    /// The discovery document with the provider's keys, fetched again once it's old.
    async fn metadata(&self) -> anyhow::Result<Arc<CoreProviderMetadata>> {
        let mut cached = self.metadata.lock().await;
        if let Some((at, metadata)) = cached.as_ref()
            && at.elapsed() < METADATA_TTL
        {
            return Ok(metadata.clone());
        }
        let metadata = Arc::new(
            CoreProviderMetadata::discover_async(self.issuer.clone(), &self.http)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("discovery at {}: {}", self.issuer.as_str(), chain(&e))
                })?,
        );
        *cached = Some((Instant::now(), metadata.clone()));
        Ok(metadata)
    }

    /// Forgets the discovery document, so that keys the provider rotated are fetched next time.
    async fn forget_metadata(&self) {
        *self.metadata.lock().await = None;
    }

    fn client(&self, metadata: &CoreProviderMetadata) -> Client {
        CoreClient::from_provider_metadata(
            metadata.clone(),
            ClientId::new(self.config.client_id.clone()),
            Some(self.config.client_secret.clone()),
        )
        .set_redirect_uri(self.redirect.clone())
    }

    fn put_flow(&self, key: String, flow: Flow) -> ApiResult<()> {
        let mut flows = self.flows.lock().expect("flow lock");
        let now = Instant::now();
        flows.retain(|_, f| f.expires > now);
        if flows.len() >= MAX_FLOWS {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too many sign-ins in progress; try again in a few minutes",
            ));
        }
        flows.insert(key, flow);
        Ok(())
    }

    /// Takes a sign-in out, so its state, nonce and code verifier can be used only once.
    fn take_flow(&self, key: &str) -> Option<Flow> {
        self.flows
            .lock()
            .expect("flow lock")
            .remove(key)
            .filter(|f| f.expires > Instant::now())
    }
}

/// The error and its causes on one line.
fn chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        text.push_str(": ");
        text.push_str(&s.to_string());
        source = s.source();
    }
    text
}

/// reqwest 0.13 as the openidconnect crate's HTTP client, with a cap on the response size.
struct Http(reqwest::Client);

impl<'c> AsyncHttpClient<'c> for Http {
    type Error = HttpError;
    type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send + 'c>>;

    fn call(&'c self, request: HttpRequest) -> Self::Future {
        Box::pin(async move {
            let mut response = self.0.execute(request.try_into()?).await?;
            let mut builder = axum::http::Response::builder().status(response.status());
            for (name, value) in response.headers() {
                builder = builder.header(name, value);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                    return Err(HttpError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            builder
                .body(body)
                .map_err(|e| HttpError::Response(e.to_string()))
        })
    }
}

#[derive(Debug)]
enum HttpError {
    Reqwest(reqwest::Error),
    TooLarge,
    Response(String),
}

impl From<reqwest::Error> for HttpError {
    fn from(e: reqwest::Error) -> Self {
        HttpError::Reqwest(e)
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Reqwest(e) => write!(f, "{}", chain(e)),
            HttpError::TooLarge => write!(f, "the answer is over {MAX_RESPONSE_BYTES} bytes"),
            HttpError::Response(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HttpError {}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/oidc/start", post(start))
        .route("/oidc/callback", get(callback))
}

/// Linked identities, behind `require_session`.
pub fn identity_routes() -> Router<AppState> {
    Router::new()
        .route("/sso/identities", get(list_identities))
        .route("/sso/identities/{id}", delete(delete_identity))
}

fn oidc(state: &AppState) -> ApiResult<&Oidc> {
    state.oidc.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "single sign-on isn't set up on this server",
        )
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum StartPurpose {
    SignIn,
    Link,
    Enroll,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Start {
    purpose: StartPurpose,
    /// The token from an enrolment link, for `enroll`.
    enroll_token: Option<String>,
}

/// Starts a sign-in: answers with the provider's URL for the GUI to go to, and sets the cookie
/// that ties the callback to this browser. A JSON `POST`, like the rest of the API, so another
/// site can't start one in the user's name.
async fn start(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<Start>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let oidc = oidc(&state)?;
    let purpose = match req.purpose {
        StartPurpose::SignIn => Purpose::SignIn,
        StartPurpose::Link => Purpose::Link {
            session_hash: auth::session_hash(&state, &jar)
                .await?
                .ok_or_else(|| ApiError::unauthorized("sign in first"))?,
        },
        StartPurpose::Enroll => {
            let token = req
                .enroll_token
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("an enrolment link's token is needed"))?;
            Purpose::Enroll {
                enroll_hash: auth::check_enroll_token(&state, token).await?,
            }
        }
    };
    let metadata = oidc.metadata().await.map_err(|e| {
        tracing::warn!("single sign-on provider: {e:#}");
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "the single sign-on provider can't be reached; see the server log",
        )
    })?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, csrf, nonce) = oidc
        .client(&metadata)
        .authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scopes(oidc.config.scopes.iter().cloned().map(Scope::new))
        .set_pkce_challenge(challenge)
        .url();
    let token = random_token("");
    oidc.put_flow(
        sha256_hex(&token),
        Flow {
            state: csrf.secret().clone(),
            nonce,
            pkce_verifier: verifier.secret().clone(),
            purpose,
            expires: Instant::now() + FLOW_TIMEOUT,
        },
    )?;
    let cookie = Cookie::build((FLOW_COOKIE, token))
        .path(FLOW_COOKIE_PATH)
        .http_only(true)
        // Lax, not Strict: the browser has to send it on the redirect back from the provider.
        .same_site(SameSite::Lax)
        .secure(state.config.cookie_secure)
        .max_age(time::Duration::seconds(FLOW_TIMEOUT.as_secs() as i64));
    Ok((jar.add(cookie), Json(json!({ "url": url.to_string() }))))
}

#[derive(Deserialize)]
struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Where the provider sends the browser back to. Always answers with a redirect into the GUI,
/// carrying `?sso=<code>` when it didn't work out.
async fn callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<Callback>,
) -> Response {
    let flow = match (&state.oidc, jar.get(FLOW_COOKIE)) {
        (Some(oidc), Some(cookie)) => oidc.take_flow(&sha256_hex(cookie.value())),
        _ => None,
    };
    let jar = jar.remove(Cookie::build(FLOW_COOKIE).path(FLOW_COOKIE_PATH));
    let page = match flow.as_ref().map(|f| &f.purpose) {
        Some(Purpose::Link { .. }) => "/settings",
        _ => "/",
    };
    let (jar, location) = match finish(&state, flow, query).await {
        Ok(true) => match auth::start_session(&state, jar.clone()).await {
            Ok(jar) => (jar, "/".to_string()),
            Err(_) => (jar, format!("/?sso={}", Failure::Failed.code())),
        },
        Ok(false) => (jar, format!("{page}?sso=linked")),
        Err(failure) => (jar, format!("{page}?sso={}", failure.code())),
    };
    let mut res = (jar, Redirect::to(&location)).into_response();
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// Checks the callback and does what the sign-in was for. `Ok(true)` means a session should start.
async fn finish(state: &AppState, flow: Option<Flow>, query: Callback) -> Result<bool, Failure> {
    let (Some(oidc), Some(flow)) = (&state.oidc, flow) else {
        return Err(Failure::Expired);
    };
    if let Some(error) = &query.error {
        tracing::warn!(
            error,
            description = query.error_description.as_deref().unwrap_or(""),
            "the single sign-on provider refused the sign-in"
        );
        return Err(Failure::Denied);
    }
    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        tracing::warn!("single sign-on callback without a code or state");
        return Err(Failure::Failed);
    };
    if !same(returned_state.as_bytes(), flow.state.as_bytes()) {
        tracing::warn!("single sign-on callback with the wrong state");
        return Err(Failure::Failed);
    }
    let identity = match verify(oidc, &flow, code).await {
        Ok(identity) => identity,
        Err(e) => {
            tracing::warn!("single sign-on: {e:#}");
            oidc.forget_metadata().await;
            return Err(Failure::Failed);
        }
    };
    let db_error = |e: sqlx::Error| {
        tracing::error!("single sign-on: database: {e}");
        Failure::Failed
    };
    match flow.purpose {
        Purpose::SignIn => {
            let found = sqlx::query(
                "UPDATE oidc_identities SET last_used_at = ? WHERE issuer = ? AND subject = ?",
            )
            .bind(now_ms())
            .bind(&identity.issuer)
            .bind(&identity.subject)
            .execute(&state.db)
            .await
            .map_err(db_error)?
            .rows_affected();
            if found == 0 {
                tracing::warn!(
                    issuer = identity.issuer,
                    subject = identity.subject,
                    label = identity.label,
                    "single sign-on by an identity that isn't linked"
                );
                return Err(Failure::Unlinked);
            }
            tracing::info!(label = identity.label, "signed in with single sign-on");
            Ok(true)
        }
        Purpose::Link { session_hash } => {
            if !auth::session_hash_live(&state.db, &session_hash)
                .await
                .map_err(db_error)?
            {
                return Err(Failure::SignedOut);
            }
            link(&state.db, &identity).await.map_err(db_error)?;
            tracing::info!(label = identity.label, "single sign-on linked");
            Ok(false)
        }
        Purpose::Enroll { enroll_hash } => {
            let mut tx = state.db.begin().await.map_err(db_error)?;
            if !auth::use_enroll_token(&mut tx, &enroll_hash)
                .await
                .map_err(db_error)?
            {
                return Err(Failure::Expired);
            }
            link(&mut *tx, &identity).await.map_err(db_error)?;
            tx.commit().await.map_err(db_error)?;
            tracing::info!(
                label = identity.label,
                "single sign-on linked through enrolment"
            );
            Ok(true)
        }
    }
}

async fn link<'e>(db: impl sqlx::SqliteExecutor<'e>, identity: &Identity) -> sqlx::Result<()> {
    let now = now_ms();
    sqlx::query(
        "INSERT INTO oidc_identities (issuer, subject, label, created_at, last_used_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (issuer, subject) DO UPDATE SET label = excluded.label, last_used_at = excluded.last_used_at",
    )
    .bind(&identity.issuer)
    .bind(&identity.subject)
    .bind(&identity.label)
    .bind(now)
    .bind(now)
    .execute(db)
    .await
    .map(|_| ())
}

/// Swaps the code for tokens, with the PKCE verifier, and checks the ID token.
async fn verify(oidc: &Oidc, flow: &Flow, code: String) -> anyhow::Result<Identity> {
    let metadata = oidc.metadata().await?;
    let client = oidc.client(&metadata);
    let tokens = client
        .exchange_code(AuthorizationCode::new(code))?
        .set_pkce_verifier(PkceCodeVerifier::new(flow.pkce_verifier.clone()))
        .request_async(&oidc.http)
        .await
        .map_err(|e| anyhow::anyhow!("token request: {}", chain(&e)))?;
    let id_token = tokens
        .extra_fields()
        .id_token()
        .ok_or_else(|| anyhow::anyhow!("the provider sent no ID token"))?;
    let verifier = client.id_token_verifier();
    let claims = id_token
        .claims(&verifier, &flow.nonce)
        .map_err(|e| anyhow::anyhow!("ID token: {}", chain(&e)))?;
    if let Some(expected) = claims.access_token_hash() {
        let actual = AccessTokenHash::from_token(
            tokens.access_token(),
            id_token.signing_alg()?,
            id_token.signing_key(&verifier)?,
        )?;
        anyhow::ensure!(
            actual == *expected,
            "ID token: the access token hash is wrong"
        );
    }
    let subject = claims.subject().as_str().to_string();
    let label = claims
        .preferred_username()
        .map(|u| u.as_str().to_string())
        .or_else(|| claims.email().map(|e| e.as_str().to_string()))
        .or_else(|| {
            claims
                .name()
                .and_then(|n| n.get(None))
                .map(|n| n.as_str().to_string())
        })
        .unwrap_or_else(|| subject.clone());
    Ok(Identity {
        issuer: claims.issuer().as_str().to_string(),
        subject,
        label,
    })
}

/// Compares in constant time.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn list_identities(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let rows: Vec<(i64, String, String, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, issuer, subject, label, created_at, last_used_at FROM oidc_identities ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await?;
    let current = state.oidc.as_ref().map(|o| o.issuer());
    let list: Vec<Value> = rows
        .into_iter()
        .map(|(id, issuer, subject, label, created, used)| {
            json!({
                "id": id,
                "issuer": issuer,
                "subject": subject,
                "label": label,
                "createdAt": created,
                "lastUsedAt": used,
                // An identity from a provider that isn't configured any more can't sign in.
                "active": current == Some(issuer.as_str()),
            })
        })
        .collect();
    Ok(Json(json!(list)))
}

async fn delete_identity(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    if auth::ways_in(&state, None, Some(id)).await? == 0 {
        return Err(ApiError::bad_request(
            "that's the last way to sign in; add a passkey or link another identity before removing it",
        ));
    }
    sqlx::query("DELETE FROM oidc_identities WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    //! The whole flow through the real router, against a small provider on a local port that
    //! signs ID tokens with a fresh Ed25519 key and checks the PKCE verifier and client secret.

    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::extract::{Form, State};
    use axum::http::{HeaderMap, Request, StatusCode, header};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use base64::Engine;
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    use openidconnect::core::{
        CoreEdDsaPrivateSigningKey, CoreIdToken, CoreIdTokenClaims, CoreJwsSigningAlgorithm,
    };
    use openidconnect::{
        AccessToken, Audience, EmptyAdditionalClaims, EndUserUsername, IssuerUrl, JsonWebKeySet,
        Nonce, PrivateSigningKey, StandardClaims, SubjectIdentifier,
    };
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    use crate::config::{Config, OidcConfig};
    use crate::state::AppState;
    use crate::util::{now_ms, sha256_hex};

    const CLIENT_ID: &str = "switchboard";
    const CLIENT_SECRET: &str = "test-client-secret";

    /// What the provider puts in the ID token for an authorization code.
    struct Grant {
        sub: String,
        nonce: String,
        challenge: String,
    }

    struct Provider {
        url: String,
        key: CoreEdDsaPrivateSigningKey,
        grants: Mutex<HashMap<String, Grant>>,
    }

    impl Provider {
        async fn start() -> Arc<Provider> {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            // A PKCS#8 Ed25519 key is this fixed prefix and the 32-byte seed.
            let mut der = hex::decode("302e020100300506032b657004220420").unwrap();
            let mut seed = [0u8; 32];
            rand::fill(&mut seed);
            der.extend_from_slice(&seed);
            let pem = format!(
                "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
                STANDARD.encode(der)
            );
            let provider = Arc::new(Provider {
                url,
                key: CoreEdDsaPrivateSigningKey::from_ed25519_pem(&pem, None).unwrap(),
                grants: Mutex::new(HashMap::new()),
            });
            let app = Router::new()
                .route("/.well-known/openid-configuration", get(discovery))
                .route("/jwks", get(jwks))
                .route("/token", post(token))
                .with_state(provider.clone());
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            provider
        }

        fn grant(&self, code: &str, sub: &str, nonce: &str, challenge: &str) {
            self.grants.lock().unwrap().insert(
                code.to_string(),
                Grant {
                    sub: sub.to_string(),
                    nonce: nonce.to_string(),
                    challenge: challenge.to_string(),
                },
            );
        }
    }

    async fn discovery(State(p): State<Arc<Provider>>) -> Json<Value> {
        Json(json!({
            "issuer": p.url,
            "authorization_endpoint": format!("{}/authorize", p.url),
            "token_endpoint": format!("{}/token", p.url),
            "jwks_uri": format!("{}/jwks", p.url),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["EdDSA"],
        }))
    }

    async fn jwks(State(p): State<Arc<Provider>>) -> Json<Value> {
        Json(serde_json::to_value(JsonWebKeySet::new(vec![p.key.as_verification_key()])).unwrap())
    }

    async fn token(
        State(p): State<Arc<Provider>>,
        headers: HeaderMap,
        Form(form): Form<HashMap<String, String>>,
    ) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
        let refuse = |error: &str| (StatusCode::BAD_REQUEST, Json(json!({ "error": error })));
        let basic = format!(
            "Basic {}",
            STANDARD.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}"))
        );
        if headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            != Some(&basic)
        {
            return Err(refuse("invalid_client"));
        }
        let grant = p
            .grants
            .lock()
            .unwrap()
            .remove(&form["code"])
            .ok_or_else(|| refuse("invalid_grant"))?;
        let verifier = form
            .get("code_verifier")
            .ok_or_else(|| refuse("invalid_grant"))?;
        if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != grant.challenge {
            return Err(refuse("invalid_grant"));
        }
        let now = chrono::Utc::now();
        let claims = CoreIdTokenClaims::new(
            IssuerUrl::new(p.url.clone()).unwrap(),
            vec![Audience::new(CLIENT_ID.into())],
            now + chrono::Duration::minutes(5),
            now,
            StandardClaims::new(SubjectIdentifier::new(grant.sub))
                .set_preferred_username(Some(EndUserUsername::new("alex".into()))),
            EmptyAdditionalClaims {},
        )
        .set_nonce(Some(Nonce::new(grant.nonce)));
        let access = AccessToken::new("access-token".into());
        let id_token = CoreIdToken::new(
            claims,
            &p.key,
            CoreJwsSigningAlgorithm::EdDsa,
            Some(&access),
            None,
        )
        .unwrap();
        Ok(Json(json!({
            "access_token": access.secret(),
            "token_type": "bearer",
            "expires_in": 300,
            "id_token": id_token.to_string(),
        })))
    }

    async fn setup() -> (Arc<Provider>, AppState, Router, std::path::PathBuf) {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let provider = Provider::start().await;
        let path =
            std::env::temp_dir().join(format!("switchboard-test-{}.db", uuid::Uuid::now_v7()));
        let db = crate::db::connect(&path).await.unwrap();
        let config = Config {
            listen: String::new(),
            db_path: path.clone(),
            web_dir: Default::default(),
            retention_days: 30,
            concurrency: 1,
            cookie_secure: false,
            origin: "http://localhost:8080".into(),
            passkey_login: false,
            oidc: Some(OidcConfig {
                issuer: provider.url.clone(),
                client_id: CLIENT_ID.into(),
                client_secret: openidconnect::ClientSecret::new(CLIENT_SECRET.into()),
                scopes: vec!["email".into()],
                ca_file: None,
                button_label: "Sign in with Test".into(),
            }),
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = AppState::new(config, db, Default::default(), tx).unwrap();
        let app = crate::api::router(state.clone());
        (provider, state, app, path)
    }

    async fn sign_in_session(state: &AppState, token: &str) {
        sqlx::query("INSERT INTO sessions VALUES (?, 0, 99999999999999)")
            .bind(sha256_hex(token))
            .execute(&state.db)
            .await
            .unwrap();
    }

    fn set_cookies(res: &axum::response::Response) -> Vec<String> {
        res.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect()
    }

    /// What a started sign-in hands the browser: its cookie, and the provider's URL to go to.
    struct Started {
        cookie: String,
        state: String,
        nonce: String,
        challenge: String,
    }

    async fn start(app: &Router, body: Value, session: Option<&str>) -> Result<Started, u16> {
        let mut req =
            Request::post("/api/auth/oidc/start").header(header::CONTENT_TYPE, "application/json");
        if let Some(s) = session {
            req = req.header(header::COOKIE, format!("switchboard_session={s}"));
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        if res.status() != StatusCode::OK {
            return Err(res.status().as_u16());
        }
        let cookies = set_cookies(&res);
        let flow = cookies
            .iter()
            .find(|c| c.starts_with("switchboard_oidc="))
            .expect("a flow cookie");
        for attribute in [
            "HttpOnly",
            "SameSite=Lax",
            "Path=/api/auth/oidc",
            "Max-Age=600",
        ] {
            assert!(flow.contains(attribute), "{flow} has {attribute}");
        }
        let cookie = flow.split(';').next().unwrap().to_string();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let url = url::Url::parse(body["url"].as_str().unwrap()).unwrap();
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["client_id"], CLIENT_ID);
        assert_eq!(
            q["redirect_uri"],
            "http://localhost:8080/api/auth/oidc/callback"
        );
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["scope"], "openid email");
        Ok(Started {
            cookie,
            state: q["state"].clone(),
            nonce: q["nonce"].clone(),
            challenge: q["code_challenge"].clone(),
        })
    }

    /// The browser coming back from the provider. Returns where it's sent and the cookies set.
    async fn callback(app: &Router, cookie: Option<&str>, query: &str) -> (String, Vec<String>) {
        let mut req = Request::get(format!("/api/auth/oidc/callback?{query}"));
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let location = res.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_string();
        (location, set_cookies(&res))
    }

    /// Starts and completes a sign-in as `sub`.
    async fn round_trip(
        provider: &Provider,
        app: &Router,
        body: Value,
        session: Option<&str>,
        sub: &str,
    ) -> (String, Vec<String>) {
        let s = start(app, body, session).await.unwrap();
        provider.grant("code-1", sub, &s.nonce, &s.challenge);
        callback(
            app,
            Some(&s.cookie),
            &format!("code=code-1&state={}", s.state),
        )
        .await
    }

    fn has_session(cookies: &[String]) -> bool {
        cookies.iter().any(|c| {
            c.starts_with("switchboard_session=") && !c.starts_with("switchboard_session=;")
        })
    }

    async fn count(state: &AppState, table: &str) -> i64 {
        // Only ever called with a table name written in the tests.
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
            .fetch_one(&state.db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn unlinked_identity_gets_no_session() {
        let (provider, state, app, path) = setup().await;
        let (location, cookies) = round_trip(
            &provider,
            &app,
            json!({ "purpose": "signIn" }),
            None,
            "user-1",
        )
        .await;
        assert_eq!(location, "/?sso=unlinked");
        assert!(!has_session(&cookies), "{cookies:?}");
        assert_eq!(count(&state, "sessions").await, 0);
        assert_eq!(count(&state, "oidc_identities").await, 0);
        std::fs::remove_file(path).ok();
    }

    #[tokio::test]
    async fn wrong_state_or_nonce_is_rejected() {
        let (provider, state, app, path) = setup().await;
        // Linked, so that only the state or nonce can be what's wrong.
        sqlx::query("INSERT INTO oidc_identities (issuer, subject, label, created_at) VALUES (?, 'user-1', 'alex', 0)")
            .bind(state.oidc.as_ref().unwrap().issuer())
            .execute(&state.db)
            .await
            .unwrap();

        let s = start(&app, json!({ "purpose": "signIn" }), None)
            .await
            .unwrap();
        provider.grant("code-1", "user-1", &s.nonce, &s.challenge);
        let (location, cookies) =
            callback(&app, Some(&s.cookie), "code=code-1&state=not-the-state").await;
        assert_eq!(location, "/?sso=failed");
        assert!(!has_session(&cookies));
        // The sign-in is used up by the failed attempt.
        let query = format!("code=code-1&state={}", s.state);
        let (location, _) = callback(&app, Some(&s.cookie), &query).await;
        assert_eq!(location, "/?sso=expired");

        let s = start(&app, json!({ "purpose": "signIn" }), None)
            .await
            .unwrap();
        provider.grant("code-2", "user-1", "another-nonce", &s.challenge);
        let query = format!("code=code-2&state={}", s.state);
        let (location, cookies) = callback(&app, Some(&s.cookie), &query).await;
        assert_eq!(location, "/?sso=failed");
        assert!(!has_session(&cookies));

        // Without the cookie from the browser that started it, nothing to finish.
        let s = start(&app, json!({ "purpose": "signIn" }), None)
            .await
            .unwrap();
        provider.grant("code-3", "user-1", &s.nonce, &s.challenge);
        let query = format!("code=code-3&state={}", s.state);
        assert_eq!(callback(&app, None, &query).await.0, "/?sso=expired");
        let other = "switchboard_oidc=someone-elses";
        assert_eq!(callback(&app, Some(other), &query).await.0, "/?sso=expired");

        // A refusal from the provider.
        let s = start(&app, json!({ "purpose": "signIn" }), None)
            .await
            .unwrap();
        let query = format!("error=access_denied&state={}", s.state);
        assert_eq!(
            callback(&app, Some(&s.cookie), &query).await.0,
            "/?sso=denied"
        );

        assert_eq!(count(&state, "sessions").await, 0);
        std::fs::remove_file(path).ok();
    }

    #[tokio::test]
    async fn links_from_settings_and_then_signs_in() {
        let (provider, state, app, path) = setup().await;
        assert_eq!(
            start(&app, json!({ "purpose": "link" }), None).await.err(),
            Some(401),
            "linking needs a session"
        );

        sign_in_session(&state, "testsession").await;
        // The Strict session cookie isn't sent on the redirect back, and isn't needed.
        let (location, cookies) = round_trip(
            &provider,
            &app,
            json!({ "purpose": "link" }),
            Some("testsession"),
            "user-1",
        )
        .await;
        assert_eq!(location, "/settings?sso=linked");
        assert!(!has_session(&cookies));
        let (issuer, subject, label): (String, String, String) =
            sqlx::query_as("SELECT issuer, subject, label FROM oidc_identities")
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(issuer, provider.url);
        assert_eq!((subject.as_str(), label.as_str()), ("user-1", "alex"));

        let (location, cookies) = round_trip(
            &provider,
            &app,
            json!({ "purpose": "signIn" }),
            None,
            "user-1",
        )
        .await;
        assert_eq!(location, "/");
        assert!(has_session(&cookies), "{cookies:?}");
        assert_eq!(count(&state, "sessions").await, 2);

        // Another identity at the same provider, with the same username and email, is not linked.
        let (location, _) = round_trip(
            &provider,
            &app,
            json!({ "purpose": "signIn" }),
            None,
            "user-2",
        )
        .await;
        assert_eq!(location, "/?sso=unlinked");

        // A link started by a session that has since signed out isn't made.
        sign_in_session(&state, "second").await;
        let s = start(&app, json!({ "purpose": "link" }), Some("second"))
            .await
            .unwrap();
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(sha256_hex("second"))
            .execute(&state.db)
            .await
            .unwrap();
        provider.grant("code-9", "user-3", &s.nonce, &s.challenge);
        let query = format!("code=code-9&state={}", s.state);
        let (location, _) = callback(&app, Some(&s.cookie), &query).await;
        assert_eq!(location, "/settings?sso=signed-out");
        assert_eq!(count(&state, "oidc_identities").await, 1);

        // Passkey sign-in is off and there are no passkeys, so the only identity can't be removed.
        let res = app
            .clone()
            .oneshot(
                Request::delete("/api/sso/identities/1")
                    .header(header::COOKIE, "switchboard_session=testsession")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert_eq!(count(&state, "oidc_identities").await, 1);
        std::fs::remove_file(path).ok();
    }

    #[tokio::test]
    async fn links_through_an_enrolment_link() {
        let (provider, state, app, path) = setup().await;
        sqlx::query("INSERT INTO enroll_tokens (token_hash, expires_at) VALUES (?, ?)")
            .bind(sha256_hex("enr_test"))
            .bind(now_ms() + 60_000)
            .execute(&state.db)
            .await
            .unwrap();
        assert_eq!(
            start(
                &app,
                json!({ "purpose": "enroll", "enrollToken": "enr_wrong" }),
                None
            )
            .await
            .err(),
            Some(401)
        );
        let body = json!({ "purpose": "enroll", "enrollToken": "enr_test" });
        let (location, cookies) = round_trip(&provider, &app, body.clone(), None, "user-1").await;
        assert_eq!(location, "/");
        assert!(has_session(&cookies));
        assert_eq!(count(&state, "oidc_identities").await, 1);
        assert_eq!(
            count(&state, "enroll_tokens").await,
            0,
            "the link is used up"
        );
        assert_eq!(start(&app, body, None).await.err(), Some(401));
        std::fs::remove_file(path).ok();
    }

    #[tokio::test]
    async fn status_shows_the_sign_in_options() {
        let (_provider, _state, app, path) = setup().await;
        let res = app
            .clone()
            .oneshot(
                Request::get("/api/auth/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status["passkeyLogin"], false);
        assert_eq!(status["sso"]["label"], "Sign in with Test");
        let res = app
            .clone()
            .oneshot(
                Request::post("/api/auth/login/start")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::FORBIDDEN,
            "passkey sign-in is off"
        );
        std::fs::remove_file(path).ok();
    }
}
