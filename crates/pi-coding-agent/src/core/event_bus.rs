use std::collections::HashMap;
use std::sync::Mutex;

pub trait EventBus: Send + Sync {
    fn emit(&self, channel: &str, data: serde_json::Value);
    fn on(
        &self,
        channel: &str,
        handler: Box<dyn Fn(serde_json::Value) + Send + Sync>,
    ) -> Box<dyn Fn() + Send + Sync>;
}

type HandlerFn = Box<dyn Fn(serde_json::Value) + Send + Sync>;

pub struct EventBusController {
    handlers: Mutex<HashMap<String, Vec<HandlerFn>>>,
}

impl EventBusController {
    pub fn new() -> Self {
        Self {
            handlers: Mutex::new(HashMap::new()),
        }
    }

    pub fn clear(&self) {
        let mut handlers = self.handlers.lock().unwrap();
        handlers.clear();
    }
}

impl Default for EventBusController {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus for EventBusController {
    fn emit(&self, channel: &str, data: serde_json::Value) {
        let handlers = self.handlers.lock().unwrap();
        if let Some(channel_handlers) = handlers.get(channel) {
            for handler in channel_handlers {
                handler(data.clone());
            }
        }
    }

    fn on(
        &self,
        channel: &str,
        handler: Box<dyn Fn(serde_json::Value) + Send + Sync>,
    ) -> Box<dyn Fn() + Send + Sync> {
        let mut handlers = self.handlers.lock().unwrap();
        handlers
            .entry(channel.to_string())
            .or_insert_with(Vec::new)
            .push(handler);
        let channel = channel.to_string();
        Box::new(move || {
            // Unsubscribe is a no-op in this simplified implementation.
            // A full implementation would remove the specific handler.
            let _ = &channel;
        })
    }
}
