use super::{Ctx, Hub, KEYRING_SERVICE, TOKEN_ENV, URL_ENV, clean, hub_error};
use crate::output::print_json;
use anyhow::{Result, anyhow, bail};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use url::Url;

const CLIENT_ID: &str = "porter-cli";
const GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// RFC 8628 device flow against Better Auth's `/api/auth/device/*`.
pub async fn login(ctx: &Ctx, origin: Url) -> Result<i32> {
    let hub = Hub::anonymous(origin)?;
    let account = hub.origin_str();
    let (st, v) = hub
        .call(
            Method::POST,
            &["api", "auth", "device", "code"],
            Some(&json!({ "client_id": CLIENT_ID })),
        )
        .await?;
    if st != StatusCode::OK {
        bail!("{}", device_error(st, &v));
    }
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let (device_code, user_code) = (s("device_code"), s("user_code"));
    if device_code.is_empty() || user_code.is_empty() {
        bail!("the hub's device-code response is missing fields");
    }
    let verify = s("verification_uri_complete");
    let verify = if verify.is_empty() {
        s("verification_uri")
    } else {
        verify
    };
    let n = |k: &str, d: u64| v.get(k).and_then(Value::as_u64).unwrap_or(d);
    let mut interval = n("interval", 5).max(1);
    let expires = Instant::now() + Duration::from_secs(n("expires_in", 1800));
    if ctx.json {
        print_json(&json!({
            "v": 1, "event": "hub-device-code", "user_code": user_code,
            "verification_uri": s("verification_uri"), "verification_uri_complete": verify,
            "expires_in": n("expires_in", 1800),
        }));
    }
    eprintln!("Open {verify} and enter the code {user_code} to sign in to {account}.");
    let body = json!({ "grant_type": GRANT, "device_code": device_code, "client_id": CLIENT_ID });
    let token = loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if Instant::now() > expires {
            bail!("the sign-in code expired; run `porter hub login` again");
        }
        let (st, v) = hub
            .call(
                Method::POST,
                &["api", "auth", "device", "token"],
                Some(&body),
            )
            .await?;
        if st == StatusCode::OK {
            match v.get("access_token").and_then(Value::as_str) {
                Some(t) if !t.is_empty() => break t.to_string(),
                _ => bail!("the hub's token response has no access_token"),
            }
        }
        match v.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += 5,
            Some("access_denied") => bail!("sign-in was denied"),
            Some("expired_token") => {
                bail!("the sign-in code expired; run `porter hub login` again")
            }
            _ => bail!("{}", device_error(st, &v)),
        }
    };
    keyring::Entry::new(KEYRING_SERVICE, &account)
        .and_then(|e| e.set_password(&token))
        .map_err(|e| {
            anyhow!(
                "signed in, but the system keyring is unavailable ({e}), so the token could not be stored; set {TOKEN_ENV} and {URL_ENV} instead"
            )
        })?;
    if ctx.json {
        print_json(&json!({"v": 1, "event": "hub-login", "hub": account}));
    } else {
        eprintln!("signed in to {account}");
    }
    Ok(0)
}

fn device_error(st: StatusCode, v: &Value) -> String {
    let d = v
        .get("error_description")
        .or_else(|| v.get("message"))
        .and_then(Value::as_str);
    match (v.get("error").and_then(Value::as_str), d) {
        (Some(e), Some(d)) => format!("hub returned {st}: {}: {}", clean(e), clean(d)),
        _ => hub_error(st, v),
    }
}

/// Ends the session on the hub (best effort), then deletes the keyring entry.
pub async fn logout(ctx: &Ctx, origin: Url) -> Result<i32> {
    let account = origin.origin().ascii_serialization();
    if let Ok(t) = keyring::Entry::new(KEYRING_SERVICE, &account).and_then(|e| e.get_password()) {
        let revoked = match Hub::with_token(origin.clone(), String::new(), &t) {
            Ok(hub) => hub
                .call(Method::POST, &["api", "auth", "sign-out"], Some(&json!({})))
                .await
                .is_ok_and(|(st, _)| st.is_success()),
            Err(_) => false,
        };
        if !revoked {
            eprintln!(
                "warning: the hub did not confirm the sign-out; the token is removed locally only"
            );
        }
    }
    let removed =
        match keyring::Entry::new(KEYRING_SERVICE, &account).and_then(|e| e.delete_credential()) {
            Ok(()) => true,
            Err(keyring::Error::NoEntry) => false,
            Err(e) => bail!("the system keyring is unavailable ({e})"),
        };
    if ctx.json {
        print_json(&json!({"v": 1, "event": "hub-logout", "hub": account, "removed": removed}));
    } else if removed {
        eprintln!("signed out of {account}");
    } else {
        eprintln!("no stored token for {account}");
    }
    if std::env::var(TOKEN_ENV).is_ok_and(|t| !t.is_empty()) {
        eprintln!("note: {TOKEN_ENV} is set and still signs you in");
    }
    Ok(0)
}

pub async fn whoami(ctx: &Ctx, hub: &Hub) -> Result<i32> {
    let (st, v) = hub
        .call(Method::GET, &["api", "auth", "get-session"], None)
        .await?;
    if st != StatusCode::OK {
        bail!("{}", hub_error(st, &v));
    }
    let Some(user) = v.get("user").filter(|u| u.is_object()) else {
        bail!(
            "not signed in to {} (the token is invalid or expired); run `porter hub login`",
            hub.origin_str()
        );
    };
    let s = |k: &str| user.get(k).and_then(Value::as_str).unwrap_or_default();
    if ctx.json {
        print_json(&json!({
            "v": 1, "event": "hub-whoami", "hub": hub.origin_str(),
            "user": {"id": s("id"), "email": s("email"), "name": s("name")},
        }));
    } else {
        println!("{} ({}) on {}", s("email"), s("name"), hub.origin_str());
    }
    Ok(0)
}
