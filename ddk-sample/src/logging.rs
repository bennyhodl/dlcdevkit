
use tracing::Subscriber;
use tracing_subscriber::Layer;
use colorful::{Color, Colorful};

pub struct SampleLogger {
    pub tx: std::sync::mpsc::Sender<String>
}

impl<S> Layer<S> for SampleLogger where S: Subscriber {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        if let Some(log_msg) = visitor.message {
            let log = format!("{}", log_msg);
            let _ = self.tx.send(log);
        }
    }
}

#[derive(Default)]
struct MessageVisitor {
    pub message: Option<String>,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{:?}", value));
        }
    }
}
