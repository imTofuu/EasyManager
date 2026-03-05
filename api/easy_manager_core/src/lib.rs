pub mod packets;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum AccountIdentifier {
	Email { email: String },
	Username { username: String },
}
