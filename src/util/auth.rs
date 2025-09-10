use std::collections::HashMap;
use crate::adapters::{AuthConfig, AuthType};

/// Build AuthConfig from metadata key-values collected during init
pub fn build_auth_config_from_metadata(metadata: &HashMap<String, String>) -> Option<AuthConfig> {
    match metadata.get("wordpress_auth_type").map(|s| s.as_str()) {
        Some("bearer") => {
            metadata.get("wordpress_bearer_token").map(|token| {
                let mut credentials = HashMap::new();
                credentials.insert("token".to_string(), token.clone());
                AuthConfig { auth_type: AuthType::Bearer, credentials }
            })
        }
        Some("basic") => {
            let user = metadata.get("wordpress_basic_username");
            let pass = metadata.get("wordpress_basic_password");
            match (user, pass) {
                (Some(u), Some(p)) => {
                    let mut credentials = HashMap::new();
                    credentials.insert("username".to_string(), u.clone());
                    credentials.insert("password".to_string(), p.clone());
                    Some(AuthConfig { auth_type: AuthType::Basic, credentials })
                }
                _ => None,
            }
        }
        Some("apikey") => {
            let header = metadata.get("wordpress_api_key_header").cloned().unwrap_or_else(|| "X-API-Key".to_string());
            metadata.get("wordpress_api_key").map(|k| {
                let mut credentials = HashMap::new();
                credentials.insert("key".to_string(), k.clone());
                credentials.insert("header".to_string(), header);
                AuthConfig { auth_type: AuthType::ApiKey, credentials }
            })
        }
        _ => None,
    }
}


