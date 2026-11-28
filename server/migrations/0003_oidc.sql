-- Single sign-on identities from an OpenID Connect provider, linked to the one owner.
-- An identity is the provider's issuer and its `sub` for the user, never the email address.
-- Only linked identities can sign in: through Settings while signed in, or through an `enroll` link.
CREATE TABLE oidc_identities (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    issuer       TEXT NOT NULL,
    subject      TEXT NOT NULL,
    -- What to show for it, from the ID token when it was linked (username, email or name).
    label        TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    UNIQUE (issuer, subject)
);
