//! Message bus between the core and modules (spec §9: modules never touch each
//! other's internals). Multi-producer, broadcast to every subscriber.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// Events published on the bus. Extend per phase; keep payloads plain data.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    ThemeChanged { light: bool },
    ModuleToggled { id: String, enabled: bool },
    /// A module asks the overlay to open its panel.
    OpenModule { id: String },
    /// A module crashed and was stopped by the supervisor.
    ModuleFailed { id: String, reason: String },
    Shutdown,
}

#[derive(Clone, Default)]
pub struct Bus {
    subscribers: Arc<Mutex<Vec<Sender<Event>>>>,
}

impl Bus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        self.subscribers.lock().expect("bus poisoned").push(tx);
        rx
    }

    /// Deliver to every live subscriber; dropped receivers are pruned.
    pub fn publish(&self, event: Event) {
        let mut subs = self.subscribers.lock().expect("bus poisoned");
        subs.retain(|tx| tx.send(event.clone()).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcasts_and_prunes() {
        let bus = Bus::new();
        let a = bus.subscribe();
        let b = bus.subscribe();
        drop(b);
        bus.publish(Event::Shutdown);
        assert_eq!(a.try_recv().unwrap(), Event::Shutdown);
        assert_eq!(bus.subscribers.lock().unwrap().len(), 1);
    }
}
