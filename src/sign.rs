use std::{collections::HashMap, path::Path};

use base64::URL_SAFE_NO_PAD;
use ring::signature::{Ed25519KeyPair, Signature};
use serde::Serialize;
use tokio::fs;

pub enum Key {
    Ed25519(Ed25519KeyPair),
}

impl Key {
    fn ty(&self) -> &'static str {
        match self {
            Key::Ed25519(_) => "ed25519",
        }
    }

    fn sign(&self, v: &[u8]) -> Signature {
        match self {
            Key::Ed25519(key) => key.sign(v),
        }
    }
}

pub async fn load_keys(kerux_root: &Path) -> Result<HashMap<String, Key>, std::io::Error> {
    let mut entries = fs::read_dir(kerux_root.join("keys")).await?;
    let mut ret = HashMap::new();
    while let Some(key_file) = entries.next_entry().await? {
        let file_name = key_file.file_name();
        let key_name = file_name.to_str().unwrap();
        let path = key_file.path();
        let contents = fs::read(&path).await?;
        if key_name.split_once(':').unwrap().0 != "ed25519" {
            panic!();
        }
        let key = Key::Ed25519(Ed25519KeyPair::from_pkcs8(&contents).unwrap());
        ret.insert(key_name.to_owned(), key);
    }
    Ok(ret)
}

pub fn sign_json(
    object: &impl Serialize,
    keys: &HashMap<String, Key>,
) -> Result<HashMap<String, String>, serde_canonical::error::Error> {
    let json = serde_canonical::ser::to_string(&object)?;
    let res = keys.iter()
        .map(|(name, key)| {
            let signature = key.sign(json.as_bytes());
            let base64 = base64::encode_config(&signature, URL_SAFE_NO_PAD);
            (name.clone(), base64)
        })
        .collect();
    Ok(res)
}
