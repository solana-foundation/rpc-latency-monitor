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
    #[error("provider '{provider}' references unset environment variable '{var}'")]
    MissingEnv { provider: String, var: String },
    #[error("provider '{provider}' has an unterminated '${{' placeholder in its url")]
    Unterminated { provider: String },
}

pub fn resolve_endpoints(providers: &[ProviderConfig]) -> Result<Vec<ProviderEndpoint>, ResolveError> {
    providers
        .iter()
        .map(|p| resolve_one(p, &|name| std::env::var(name).ok()))
        .collect()
}

fn resolve_one(
    provider: &ProviderConfig,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<ProviderEndpoint, ResolveError> {
    let url = substitute(&provider.url, lookup).map_err(|e| e.with_provider(&provider.name))?;
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
    fn resolve_one_attaches_provider_context_to_errors() {
        let provider = ProviderConfig {
            name: "helius".to_string(),
            url: "https://x/${NOPE}".to_string(),
        };
        let err = resolve_one(&provider, &lookup(&[])).expect_err("missing env should error");
        assert!(matches!(
            err,
            ResolveError::MissingEnv { provider, var } if provider == "helius" && var == "NOPE"
        ));
    }
}
