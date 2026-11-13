use std::fmt;

use crate::config::ProviderConfig;

#[derive(Clone)]
pub struct ProviderEndpoint {
    pub name: String,
    pub url: String,
}

impl fmt::Debug for ProviderEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderEndpoint")
            .field("name", &self.name)
            .field("url", &redacted_url(&self.url))
            .finish()
    }
}

pub fn redacted_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    if scheme.is_empty() {
        host.to_string()
    } else {
        format!("{scheme}://{host}")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("'{provider}' references unset environment variable '{var}'")]
    MissingEnv { provider: String, var: String },
    #[error("'{provider}' has an unterminated '${{' placeholder in its url")]
    Unterminated { provider: String },
}

pub fn resolve_endpoints(
    providers: &[ProviderConfig],
    region: &str,
) -> Result<Vec<ProviderEndpoint>, ResolveError> {
    providers
        .iter()
        .filter(|p| serves_region(p, region))
        .map(|p| resolve_one(p, region, &|name| std::env::var(name).ok()))
        .collect()
}

pub fn serves_region(provider: &ProviderConfig, region: &str) -> bool {
    provider.geos.is_empty()
        || provider
            .geos
            .iter()
            .any(|g| g == crate::geo::geo_for(region))
}

pub const KNOWN_GEOS: &[&str] = &[
    "us-east",
    "us-west",
    "eu-central",
    "eu-west",
    "ap-northeast",
    "ap-southeast",
];

pub fn validate_geos(providers: &[ProviderConfig]) -> Result<(), String> {
    for p in providers {
        for g in &p.geos {
            if !KNOWN_GEOS.contains(&g.as_str()) {
                return Err(format!(
                    "provider '{}' lists unknown geo '{}'; known geos: {}",
                    p.name,
                    g,
                    KNOWN_GEOS.join(", ")
                ));
            }
        }
    }
    Ok(())
}

pub fn resolve_url(context: &str, url: &str) -> Result<String, ResolveError> {
    substitute(url, &|name| std::env::var(name).ok()).map_err(|e| e.with_provider(context))
}

fn resolve_one(
    provider: &ProviderConfig,
    region: &str,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<ProviderEndpoint, ResolveError> {
    let raw = provider.region_urls.get(region).unwrap_or(&provider.url);
    let url = substitute(raw, lookup).map_err(|e| e.with_provider(&provider.name))?;
    Ok(ProviderEndpoint {
        name: provider.name.clone(),
        url,
    })
}

fn substitute(
    input: &str,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<String, SubstituteError> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(SubstituteError::Unterminated);
        };
        let Some(value) = lookup(&after[..end]) else {
            return Err(SubstituteError::MissingVar(after[..end].to_string()));
        };
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

enum SubstituteError {
    MissingVar(String),
    Unterminated,
}

impl SubstituteError {
    fn with_provider(self, provider: &str) -> ResolveError {
        match self {
            Self::MissingVar(var) => ResolveError::MissingEnv {
                provider: provider.to_string(),
                var,
            },
            Self::Unterminated => ResolveError::Unterminated {
                provider: provider.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn substitutes_single_and_multiple_placeholders() {
        let lk = lookup(&[("HOST", "h.example"), ("TOKEN", "abc")]);
        assert_eq!(
            substitute("https://${HOST}/${TOKEN}", &lk).ok(),
            Some("https://h.example/abc".to_string())
        );
    }

    #[test]
    fn passes_through_urls_without_placeholders() {
        let lk = lookup(&[]);
        assert_eq!(
            substitute("https://api.mainnet-beta.solana.com", &lk).ok(),
            Some("https://api.mainnet-beta.solana.com".to_string())
        );
    }

    #[test]
    fn errors_on_missing_variable() {
        let lk = lookup(&[]);
        assert!(matches!(
            substitute("https://x/${MISSING}", &lk),
            Err(SubstituteError::MissingVar(v)) if v == "MISSING"
        ));
    }

    #[test]
    fn errors_on_unterminated_placeholder() {
        let lk = lookup(&[("A", "1")]);
        assert!(matches!(
            substitute("https://x/${A", &lk),
            Err(SubstituteError::Unterminated)
        ));
    }

    #[test]
    fn redacted_url_strips_secrets_from_query_path_and_userinfo() {
        assert_eq!(
            redacted_url("https://mainnet.helius-rpc.com/?api-key=SECRET"),
            "https://mainnet.helius-rpc.com"
        );
        assert_eq!(
            redacted_url("https://host.example/SECRET_TOKEN"),
            "https://host.example"
        );
        assert_eq!(
            redacted_url("https://user:pass@host.example:8899/x"),
            "https://host.example:8899"
        );
    }

    #[test]
    fn debug_does_not_expose_the_api_key() {
        let endpoint = ProviderEndpoint {
            name: "helius".to_string(),
            url: "https://rpc.example/?api-key=SUPERSECRET".to_string(),
        };
        let rendered = format!("{endpoint:?}");
        assert!(!rendered.contains("SUPERSECRET"));
        assert!(rendered.contains("helius"));
    }

    #[test]
    fn geos_scope_providers_to_regions() {
        let scoped = ProviderConfig {
            name: "fluxrpc".to_string(),
            url: "https://cdn.example".to_string(),
            geos: vec!["eu-central".to_string(), "us-east".to_string()],
            region_urls: Default::default(),
        };
        assert!(serves_region(&scoped, "fra"));
        assert!(serves_region(&scoped, "nyc"));
        assert!(!serves_region(&scoped, "tyo2"));
        assert!(!serves_region(&scoped, "asia-southeast1"));
        let global = ProviderConfig {
            name: "helius".to_string(),
            url: "https://x".to_string(),
            geos: Vec::new(),
            region_urls: Default::default(),
        };
        assert!(serves_region(&global, "tyo2"));
    }

    #[test]
    fn region_urls_override_the_default_url() {
        let mut region_urls = std::collections::HashMap::new();
        region_urls.insert("fra".to_string(), "https://eu.example/${K}".to_string());
        let provider = ProviderConfig {
            name: "fluxrpc".to_string(),
            url: "https://cdn.example/${K}".to_string(),
            geos: Vec::new(),
            region_urls,
        };
        let eu = resolve_one(&provider, "fra", &lookup(&[("K", "v")])).unwrap();
        assert_eq!(eu.url, "https://eu.example/v");
        let other = resolve_one(&provider, "nyc", &lookup(&[("K", "v")])).unwrap();
        assert_eq!(other.url, "https://cdn.example/v");
    }

    #[test]
    fn validate_geos_rejects_typos() {
        let provider = ProviderConfig {
            name: "fluxrpc".to_string(),
            url: "https://x".to_string(),
            geos: vec!["europe".to_string()],
            region_urls: Default::default(),
        };
        assert!(validate_geos(&[provider]).is_err());
    }

    #[test]
    fn resolve_one_attaches_provider_context_to_errors() {
        let provider = ProviderConfig {
            name: "helius".to_string(),
            url: "https://x/${NOPE}".to_string(),
            geos: Vec::new(),
            region_urls: Default::default(),
        };
        let err =
            resolve_one(&provider, "fra", &lookup(&[])).expect_err("missing env should error");
        assert!(matches!(
            err,
            ResolveError::MissingEnv { provider, var } if provider == "helius" && var == "NOPE"
        ));
    }
}
