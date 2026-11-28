use std::env;
use std::path::PathBuf;

/// Settings from the environment; see the README for the list.
#[derive(Debug, Clone)]
pub struct Config {
    pub listen: String,
    pub db_path: PathBuf,
    pub web_dir: PathBuf,
    pub retention_days: i64,
    pub concurrency: usize,
    pub cookie_secure: bool,
    /// Where the GUI is opened, such as `https://notify.devops.lan`. Passkeys are bound to its host.
    pub origin: String,
    /// Whether the sign-in page offers passkeys. Registering one through `enroll` always works.
    pub passkey_login: bool,
    /// Single sign-on through an OpenID Connect provider; off unless an issuer is set.
    pub oidc: Option<OidcConfig>,
}

/// An OpenID Connect provider to sign in with.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// The issuer, such as `https://auth.example.lan`. Endpoints come from its discovery document.
    pub issuer: String,
    pub client_id: String,
    /// Redacted in `Debug`, so a logged config can't show it.
    pub client_secret: openidconnect::ClientSecret,
    /// Scopes to ask for besides `openid`, which is always sent.
    pub scopes: Vec<String>,
    /// A PEM file of CA certificates trusted, besides the system's, for talking to the issuer.
    pub ca_file: Option<PathBuf>,
    /// The sign-in button's text, such as "Sign in with Pocket ID".
    pub button_label: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_vars(|name| env::var(name).ok().filter(|v| !v.is_empty()))
    }

    fn from_vars(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let var = |name: &str, default: &str| get(name).unwrap_or_else(|| default.to_string());
        fn parse<T: std::str::FromStr>(name: &str, value: String) -> anyhow::Result<T> {
            value
                .parse()
                .map_err(|_| anyhow::anyhow!("{name} is not valid"))
        }
        let oidc = match get("SWITCHBOARD_OIDC_ISSUER") {
            None => None,
            Some(issuer) => {
                let required = |name: &str| {
                    get(name).ok_or_else(|| {
                        anyhow::anyhow!("SWITCHBOARD_OIDC_ISSUER is set, so {name} is needed too")
                    })
                };
                let secret = match (
                    get("SWITCHBOARD_OIDC_CLIENT_SECRET"),
                    get("SWITCHBOARD_OIDC_CLIENT_SECRET_FILE"),
                ) {
                    (Some(_), Some(_)) => anyhow::bail!(
                        "set SWITCHBOARD_OIDC_CLIENT_SECRET or SWITCHBOARD_OIDC_CLIENT_SECRET_FILE, not both"
                    ),
                    (Some(secret), None) => secret,
                    (None, Some(file)) => std::fs::read_to_string(&file)
                        .map_err(|e| {
                            anyhow::anyhow!("SWITCHBOARD_OIDC_CLIENT_SECRET_FILE {file}: {e}")
                        })?
                        .trim()
                        .to_string(),
                    (None, None) => anyhow::bail!(
                        "SWITCHBOARD_OIDC_ISSUER is set, so SWITCHBOARD_OIDC_CLIENT_SECRET or SWITCHBOARD_OIDC_CLIENT_SECRET_FILE is needed too"
                    ),
                };
                anyhow::ensure!(
                    !secret.is_empty(),
                    "the OpenID Connect client secret is empty"
                );
                let scopes = var("SWITCHBOARD_OIDC_SCOPES", "openid profile email")
                    .split([' ', ','])
                    .filter(|s| !s.is_empty() && *s != "openid")
                    .map(str::to_string)
                    .collect();
                Some(OidcConfig {
                    issuer,
                    client_id: required("SWITCHBOARD_OIDC_CLIENT_ID")?,
                    client_secret: openidconnect::ClientSecret::new(secret),
                    scopes,
                    ca_file: get("SWITCHBOARD_OIDC_CA_FILE").map(PathBuf::from),
                    button_label: var("SWITCHBOARD_OIDC_BUTTON_LABEL", "Sign in with SSO"),
                })
            }
        };
        let passkey_login = parse(
            "SWITCHBOARD_PASSKEY_LOGIN",
            var("SWITCHBOARD_PASSKEY_LOGIN", "true"),
        )?;
        anyhow::ensure!(
            passkey_login || oidc.is_some(),
            "SWITCHBOARD_PASSKEY_LOGIN=false leaves no way to sign in without SWITCHBOARD_OIDC_ISSUER"
        );
        Ok(Self {
            listen: var("SWITCHBOARD_LISTEN", "0.0.0.0:8080"),
            db_path: var("SWITCHBOARD_DB", "data/switchboard.db").into(),
            web_dir: var("SWITCHBOARD_WEB_DIR", "web/dist").into(),
            retention_days: parse(
                "SWITCHBOARD_RETENTION_DAYS",
                var("SWITCHBOARD_RETENTION_DAYS", "30"),
            )?,
            concurrency: parse::<usize>(
                "SWITCHBOARD_CONCURRENCY",
                var("SWITCHBOARD_CONCURRENCY", "16"),
            )?
            .max(1),
            cookie_secure: parse(
                "SWITCHBOARD_COOKIE_SECURE",
                var("SWITCHBOARD_COOKIE_SECURE", "true"),
            )?,
            origin: var("SWITCHBOARD_ORIGIN", "http://localhost:8080")
                .trim_end_matches('/')
                .to_string(),
            passkey_login,
            oidc,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::Config;

    fn config(vars: &[(&str, &str)]) -> anyhow::Result<Config> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_vars(|name| vars.get(name).cloned())
    }

    #[test]
    fn oidc_is_off_without_an_issuer() {
        let c = config(&[]).unwrap();
        assert!(c.oidc.is_none());
        assert!(c.passkey_login);
        assert!(config(&[("SWITCHBOARD_PASSKEY_LOGIN", "false")]).is_err());
    }

    #[test]
    fn reads_oidc_settings() {
        let base = [
            ("SWITCHBOARD_OIDC_ISSUER", "https://auth.example.lan"),
            ("SWITCHBOARD_OIDC_CLIENT_ID", "switchboard"),
        ];
        assert!(config(&base).is_err(), "a secret is needed");

        let c = config(&[base[0], base[1], ("SWITCHBOARD_OIDC_CLIENT_SECRET", "s3")]).unwrap();
        let oidc = c.oidc.unwrap();
        assert_eq!(oidc.scopes, ["profile", "email"]);
        assert_eq!(oidc.button_label, "Sign in with SSO");
        assert_eq!(oidc.client_secret.secret(), "s3");
        assert!(
            !format!("{oidc:?}").contains("s3"),
            "the secret is redacted"
        );

        let file =
            std::env::temp_dir().join(format!("switchboard-secret-{}", uuid::Uuid::now_v7()));
        std::fs::write(&file, "from-file\n").unwrap();
        let path = file.to_str().unwrap();
        let c = config(&[
            base[0],
            base[1],
            ("SWITCHBOARD_OIDC_CLIENT_SECRET_FILE", path),
            ("SWITCHBOARD_OIDC_SCOPES", "openid,email groups"),
            ("SWITCHBOARD_PASSKEY_LOGIN", "false"),
        ])
        .unwrap();
        std::fs::remove_file(&file).unwrap();
        let oidc = c.oidc.unwrap();
        assert_eq!(oidc.client_secret.secret(), "from-file");
        assert_eq!(oidc.scopes, ["email", "groups"]);
        assert!(!c.passkey_login);

        assert!(
            config(&[
                base[0],
                base[1],
                ("SWITCHBOARD_OIDC_CLIENT_SECRET", "a"),
                ("SWITCHBOARD_OIDC_CLIENT_SECRET_FILE", path),
            ])
            .is_err()
        );
    }
}
