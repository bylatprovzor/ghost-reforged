use iced::{Subscription, futures::StreamExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Available(bool),
    Open,
    Quit,
}

pub fn subscription() -> Subscription<Event> {
    Subscription::run(events)
}

fn events() -> impl iced::futures::Stream<Item = Event> {
    iced::stream::channel(8, async |mut output| {
        use iced::futures::{SinkExt, channel::mpsc};
        let (tx, mut rx) = mpsc::unbounded();
        #[cfg(windows)]
        let _guard = native::start(tx.clone());
        #[cfg(not(windows))]
        let _ = tx.unbounded_send(Event::Available(false));
        drop(tx);
        while let Some(event) = rx.next().await {
            if output.send(event).await.is_err() {
                break;
            }
        }
    })
}

#[cfg(windows)]
#[path = "tray_windows.rs"]
mod native;
