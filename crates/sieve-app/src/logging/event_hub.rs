use super::TelegramLoopEvent;
use sieve_types::RuntimeEvent;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub(crate) struct RuntimeEventHub {
    subscribers: Arc<Mutex<Vec<RuntimeEventSubscriber>>>,
}

enum RuntimeEventSubscriber {
    #[cfg_attr(not(test), allow(dead_code))]
    Runtime(Sender<RuntimeEvent>),
    Telegram(Sender<TelegramLoopEvent>),
}

impl RuntimeEventHub {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn subscribe(&self) -> Receiver<RuntimeEvent> {
        let (tx, rx) = mpsc::channel();
        self.subscribers
            .lock()
            .expect("runtime event subscribers lock poisoned")
            .push(RuntimeEventSubscriber::Runtime(tx));
        rx
    }

    pub(super) fn publish(&self, event: &RuntimeEvent) {
        let Ok(mut subscribers) = self.subscribers.lock() else {
            return;
        };
        subscribers.retain(|subscriber| subscriber.publish(event));
    }
}

impl From<Sender<TelegramLoopEvent>> for RuntimeEventHub {
    fn from(sender: Sender<TelegramLoopEvent>) -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(vec![RuntimeEventSubscriber::Telegram(sender)])),
        }
    }
}

impl RuntimeEventSubscriber {
    fn publish(&self, event: &RuntimeEvent) -> bool {
        match self {
            Self::Runtime(sender) => sender.send(event.clone()).is_ok(),
            Self::Telegram(sender) => sender
                .send(TelegramLoopEvent::Runtime(Box::new(event.clone())))
                .is_ok(),
        }
    }
}
