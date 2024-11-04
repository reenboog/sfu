use rand::{rngs::OsRng, Rng};

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uid(u64);

impl Uid {
	pub fn generate() -> Self {
		let mut rng = OsRng;
		Self(rng.gen())
	}
}
