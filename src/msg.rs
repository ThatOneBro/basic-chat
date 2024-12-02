use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct CreateMessage {
    pub time: u64,
    // TODO: Remove user_id and username, or potentially just validate them against values in JWT later (to extra processing)
    pub user_id: String,
    pub username: String,
    pub text: String,
    pub channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub reply_to: Option<String>,
    // #[serde(skip_serializing_if = "Option::is_none")]
    // #[serde(default)]
    // encrypt_meta: Option<EncryptMeta>,
    // #[serde(skip_serializing_if = "Option::is_none")]
    // #[serde(default)]
    // encrypt_meta_sig: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct Message {
    pub id: String,
    pub time: u64,
    pub user_id: String,
    pub username: String,
    pub text: String,
    pub channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub reply_to: Option<String>,
    // #[serde(skip_serializing_if = "Option::is_none")]
    // #[serde(default)]
    // encrypt_meta: Option<EncryptMeta>,
    // #[serde(skip_serializing_if = "Option::is_none")]
    // #[serde(default)]
    // encrypt_meta_sig: Option<String>,
}
