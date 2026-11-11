use std::collections::HashMap;
use crate::adapters::{AuthConfig, AuthType};

/// Metadata key -> environment variable that overrides it. Env vars let servers and
/// agents pass credentials without writing them into porter.config.
const ENV_OVERRIDES: &[(&str, &str)] = &[
    ("wordpress_auth_type", "PORTER_WP_AUTH_TYPE"),
    ("wordpress_bearer_token", "PORTER_WP_BEARER_TOKEN"),
    ("wordpress_basic_username", "PORTER_WP_USERNAME"),
    ("wordpress_basic_password", "PORTER_WP_PASSWORD"),
    ("wordpress_api_key", "PORTER_WP_API_KEY"),
    ("wordpress_api_key_header", "PORTER_WP_API_KEY_HEADER"),
];

/// Build AuthConfig from metadata key-values collected during init, with PORTER_WP_* env vars taking precedence
pub fn build_auth_config_from_metadata(metadata: &HashMap<String, String>) -> Option<AuthConfig> {
    build_auth_config(|key| {
        ENV_OVERRIDES
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, env)| std::env::var(env).ok())
            .filter(|v| !v.is_empty())
            .or_else(|| metadata.get(key).cloned())
    })
}

fn build_auth_config(get: impl Fn(&str) -> Option<String>) -> Option<AuthConfig> {
    match get("wordpress_auth_type").as_deref() {
        Some("bearer") => {
            get("wordpress_bearer_token").map(|token| {
                let mut credentials = HashMap::new();
                credentials.insert("token".to_string(), token);
                AuthConfig { auth_type: AuthType::Bearer, credentials }
            })
        }
        Some("basic") => {
            match (get("wordpress_basic_username"), get("wordpress_basic_password")) {
                (Some(u), Some(p)) => {
                    let mut credentials = HashMap::new();
                    credentials.insert("username".to_string(), u);
                    credentials.insert("password".to_string(), p);
                    Some(AuthConfig { auth_type: AuthType::Basic, credentials })
                }
                _ => None,
            }
        }
        Some("apikey") => {
            let header = get("wordpress_api_key_header").unwrap_or_else(|| "X-API-Key".to_string());
            get("wordpress_api_key").map(|k| {
                let mut credentials = HashMap::new();
                credentials.insert("key".to_string(), k);
                credentials.insert("header".to_string(), header);
                AuthConfig { auth_type: AuthType::ApiKey, credentials }
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_beats_metadata() {
        let meta: HashMap<String, String> = [
            ("wordpress_auth_type", "bearer"),
            ("wordpress_bearer_token", "from-config"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let env = |key: &str| (key == "wordpress_bearer_token").then(|| "from-env".to_string());

        let auth = build_auth_config(|k| env(k).or_else(|| meta.get(k).cloned())).unwrap();
        assert_eq!(auth.credentials["token"], "from-env");

        let auth = build_auth_config(|k| meta.get(k).cloned()).unwrap();
        assert_eq!(auth.credentials["token"], "from-config");
    }
}
