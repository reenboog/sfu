use rand::{rngs::OsRng, Rng};
use serde::{
	de::{self, Visitor},
	Deserialize, Deserializer, Serialize, Serializer,
};
use std::{fmt, str::FromStr};

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uid(u64);

impl Uid {
	pub fn generate() -> Self {
		let mut rng = OsRng;
		Self(rng.gen())
	}

	pub(crate) fn as_bytes(&self) -> [u8; 8] {
		self.0.to_be_bytes()
	}

	pub(crate) fn to_base64(&self) -> String {
		base64::encode_config(&self.0.to_be_bytes(), base64::URL_SAFE)
	}
}

impl PartialEq<u64> for Uid {
	fn eq(&self, other: &u64) -> bool {
		&self.0 == other
	}
}

impl PartialEq<Uid> for u64 {
	fn eq(&self, other: &Uid) -> bool {
		self == &other.0
	}
}

impl fmt::Display for Uid {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.to_base64())
	}
}

impl FromStr for Uid {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let bytes = base64::decode_config(s, base64::URL_SAFE).map_err(|_| "decode error")?;

		if bytes.len() != 8 {
			return Err(String::from("wrong length"));
		}

		let mut array = [0u8; 8];
		array.copy_from_slice(&bytes);

		Ok(Uid(u64::from_be_bytes(array)))
	}
}

impl Serialize for Uid {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		serializer.serialize_str(&self.to_base64())
	}
}

impl<'de> Deserialize<'de> for Uid {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		struct Base64Visitor;

		impl<'de> Visitor<'de> for Base64Visitor {
			type Value = Uid;

			fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
				formatter.write_str("a base64-encoded u64 string")
			}

			fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
			where
				E: de::Error,
			{
				Ok(Uid::from_str(value).map_err(de::Error::custom)?)
			}
		}

		deserializer.deserialize_str(Base64Visitor)
	}
}
