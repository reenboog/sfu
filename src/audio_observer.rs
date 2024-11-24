use mediasoup::{
	prelude::{AudioLevelObserver, AudioLevelObserverOptions},
	router::Router,
};
use tokio::sync::mpsc::Sender;

use crate::room;

pub async fn create(room_tx: Sender<room::Event>, router: Router) -> Option<AudioLevelObserver> {
	let mut options = AudioLevelObserverOptions::default();
	// just an arbitrary values, in ms; works fine
	options.interval = 500;
	let observer = router
		.create_audio_level_observer(options)
		.await
		.ok();

	let room_tx_on_volume = room_tx.clone();
	if let Some(ref observer) = observer {
		observer
			.on_volumes(move |volumes| {
				_ = room_tx_on_volume.try_send(room::Event::OnAudioLevelsChange {
					producer: volumes.first().cloned(),
				});
			})
			.detach();
		observer
			.on_silence(move || {
				_ = room_tx.try_send(room::Event::OnAudioLevelsChange { producer: None });
			})
			.detach();
	}

	observer
}
