use std::{fs::OpenOptions, io::Write};

use tracing::Subscriber;
use tracing_subscriber::Layer;


pub struct DdkSubscriber;

impl<S: Subscriber> Layer<S> for DdkSubscriber {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        if let Some(message) = visitor.message {
            let mut file = OpenOptions::new().append(true).open("./logs.txt").unwrap();
            let msg = format!("{}\n", message);
            let _ = file.write(msg.as_bytes());
        } 
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: Option<String>,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("[ddk]:: {:?}", value));
        }
    }
}
