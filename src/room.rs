use crate::uid::Uid;
use mediasoup::worker::Worker;

pub struct Room {
	id: Uid,
}

impl Room {
	pub fn new(worker: &Worker, id: Uid) -> Self {
		Self { id }
	}
}
