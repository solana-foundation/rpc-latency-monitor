use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::config::{GpaDeriveConfig, GpaTarget, MemcmpFilter};
use crate::rpc::methods::{
    self, HIGH_VOLUME_MINTS, TOKEN_ACCOUNT_LEN, TOKEN_ACCOUNT_MINT_OFFSET,
    TOKEN_ACCOUNT_OWNER_OFFSET, TOKEN_PROGRAM,
};
use crate::rpc::RpcClient;

pub const BY_MINT_NAME: &str = "derived_token_by_mint";
pub const BY_OWNER_NAME: &str = "derived_token_by_owner";
const SHAPE_COUNT: usize = 2;
const MAX_PREFLIGHTS: usize = 12;

#[derive(Clone, Default)]
pub struct DerivedTargets(Arc<Mutex<Vec<GpaTarget>>>);

impl DerivedTargets {
    pub fn current(&self) -> Vec<GpaTarget> {
        self.0.lock().map(|guard| guard.clone()).unwrap_or_default()
    }

    pub fn update(&self, fresh: Vec<GpaTarget>) {
        let Ok(mut guard) = self.0.lock() else {
            return;
        };
        for target in fresh {
            match guard.iter_mut().find(|t| t.name == target.name) {
                Some(existing) => *existing = target,
                None => guard.push(target),
            }
        }
    }
}

pub async fn run(
    client: Arc<RpcClient>,
    config: GpaDeriveConfig,
    pool: Arc<Mutex<Vec<String>>>,
    targets: DerivedTargets,
) {
    loop {
        let accounts = pool.lock().map(|guard| guard.clone()).unwrap_or_default();
        derive_once(&client, &config, accounts, &targets).await;
        sleep(config.interval).await;
    }
}

async fn derive_once(
    client: &RpcClient,
    config: &GpaDeriveConfig,
    accounts: Vec<String>,
    targets: &DerivedTargets,
) {
    if accounts.is_empty() {
        return;
    }
    let params = json!([accounts, { "encoding": "base64", "commitment": "confirmed" }]);
    let Some(result) = client
        .raw_call(&config.endpoint, "getMultipleAccounts", params)
        .await
    else {
        return;
    };
    let mut picked: Vec<GpaTarget> = Vec::new();
    for candidate in candidate_targets(&token_anchors(&result))
        .into_iter()
        .take(MAX_PREFLIGHTS)
    {
        if picked.iter().any(|t| t.name == candidate.name) {
            continue;
        }
        let Some(count_result) = client
            .raw_call(
                &config.endpoint,
                "getProgramAccounts",
                methods::gpa_count_params(&candidate),
            )
            .await
        else {
            continue;
        };
        let Some(count) = gpa_count(&count_result) else {
            continue;
        };
        if count >= config.min_accounts && count <= config.max_accounts {
            picked.push(candidate);
            if picked.len() == SHAPE_COUNT {
                break;
            }
        }
    }
    targets.update(picked);
}

#[derive(Debug, PartialEq, Eq)]
pub struct TokenAnchor {
    pub mint: String,
    pub owner: String,
}

pub fn token_anchors(result: &Value) -> Vec<TokenAnchor> {
    let value = result.get("value").unwrap_or(result);
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    let mut out: Vec<TokenAnchor> = Vec::new();
    for entry in entries {
        if entry.get("owner").and_then(Value::as_str) != Some(TOKEN_PROGRAM) {
            continue;
        }
        let Some(b64) = entry
            .get("data")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            continue;
        };
        if bytes.len() != TOKEN_ACCOUNT_LEN as usize {
            continue;
        }
        let anchor = TokenAnchor {
            mint: bs58::encode(&bytes[0..32]).into_string(),
            owner: bs58::encode(&bytes[32..64]).into_string(),
        };
        if !out.contains(&anchor) {
            out.push(anchor);
        }
    }
    out
}

pub fn candidate_targets(anchors: &[TokenAnchor]) -> Vec<GpaTarget> {
    let mut out = Vec::new();
    for anchor in anchors {
        if !HIGH_VOLUME_MINTS.contains(&anchor.mint.as_str()) {
            out.push(token_target(
                BY_MINT_NAME,
                TOKEN_ACCOUNT_MINT_OFFSET,
                &anchor.mint,
            ));
        }
        out.push(token_target(
            BY_OWNER_NAME,
            TOKEN_ACCOUNT_OWNER_OFFSET,
            &anchor.owner,
        ));
    }
    out
}

fn token_target(name: &str, offset: u64, anchor: &str) -> GpaTarget {
    GpaTarget {
        name: name.to_owned(),
        program: TOKEN_PROGRAM.to_owned(),
        data_size: Some(TOKEN_ACCOUNT_LEN),
        memcmp: vec![MemcmpFilter {
            offset,
            bytes: anchor.to_owned(),
        }],
    }
}

pub fn gpa_count(result: &Value) -> Option<u64> {
    result
        .get("value")
        .unwrap_or(result)
        .as_array()
        .map(|a| a.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_account_entry(mint: &[u8; 32], owner: &[u8; 32]) -> Value {
        let mut data = vec![0u8; TOKEN_ACCOUNT_LEN as usize];
        data[0..32].copy_from_slice(mint);
        data[32..64].copy_from_slice(owner);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
        json!({ "owner": TOKEN_PROGRAM, "data": [b64, "base64"] })
    }

    #[test]
    fn anchors_come_only_from_well_formed_token_accounts() {
        let mint = [1u8; 32];
        let owner = [2u8; 32];
        let short = base64::engine::general_purpose::STANDARD.encode([0u8; 10]);
        let result = json!({ "value": [
            null,
            { "owner": "SomeOtherProgram", "data": ["", "base64"] },
            { "owner": TOKEN_PROGRAM, "data": [short, "base64"] },
            token_account_entry(&mint, &owner),
            token_account_entry(&mint, &owner),
        ]});
        let anchors = token_anchors(&result);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].mint, bs58::encode(mint).into_string());
        assert_eq!(anchors[0].owner, bs58::encode(owner).into_string());
        assert!(token_anchors(&json!({ "value": null })).is_empty());
    }

    #[test]
    fn candidates_cover_both_shapes_and_skip_high_volume_mints() {
        let anchors = vec![
            TokenAnchor {
                mint: HIGH_VOLUME_MINTS[0].to_owned(),
                owner: "ownerA".to_owned(),
            },
            TokenAnchor {
                mint: "rareMint".to_owned(),
                owner: "ownerB".to_owned(),
            },
        ];
        let candidates = candidate_targets(&anchors);
        let names: Vec<&str> = candidates.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec![BY_OWNER_NAME, BY_MINT_NAME, BY_OWNER_NAME]);

        let by_mint = &candidates[1];
        assert_eq!(by_mint.program, TOKEN_PROGRAM);
        assert_eq!(by_mint.data_size, Some(TOKEN_ACCOUNT_LEN));
        assert_eq!(by_mint.memcmp[0].offset, TOKEN_ACCOUNT_MINT_OFFSET);
        assert_eq!(by_mint.memcmp[0].bytes, "rareMint");
        assert_eq!(candidates[2].memcmp[0].offset, TOKEN_ACCOUNT_OWNER_OFFSET);
        assert_eq!(candidates[2].memcmp[0].bytes, "ownerB");
    }

    #[test]
    fn count_reads_bare_and_context_wrapped_arrays() {
        assert_eq!(gpa_count(&json!([{}, {}, {}])), Some(3));
        assert_eq!(
            gpa_count(&json!({ "context": { "slot": 1 }, "value": [{}] })),
            Some(1)
        );
        assert_eq!(gpa_count(&json!({ "value": null })), None);
    }

    #[test]
    fn update_replaces_same_shape_and_keeps_the_other() {
        let targets = DerivedTargets::default();
        targets.update(vec![
            token_target(BY_MINT_NAME, TOKEN_ACCOUNT_MINT_OFFSET, "mintA"),
            token_target(BY_OWNER_NAME, TOKEN_ACCOUNT_OWNER_OFFSET, "ownerA"),
        ]);
        targets.update(vec![token_target(
            BY_MINT_NAME,
            TOKEN_ACCOUNT_MINT_OFFSET,
            "mintB",
        )]);
        let current = targets.current();
        assert_eq!(current.len(), 2);
        assert_eq!(current[0].memcmp[0].bytes, "mintB");
        assert_eq!(current[1].memcmp[0].bytes, "ownerA");
    }
}
