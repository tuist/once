//! Records every event and span field emitted on the current thread, so
//! tests can assert what reaches the logs.

use std::fmt::{Debug, Write as _};
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::subscriber::{DefaultGuard, Interest};
use tracing::{span, Event, Metadata, Subscriber};

#[derive(Clone, Default)]
pub(crate) struct Captured(Arc<Mutex<String>>);

impl Captured {
    pub(crate) fn text(&self) -> String {
        self.0.lock().unwrap().clone()
    }
}

/// Capture logs on this thread until the guard drops. Tests that use it
/// should run on a current-thread runtime so spawned tasks log here too.
pub(crate) fn capture() -> (Captured, DefaultGuard) {
    let captured = Captured::default();
    let guard = tracing::subscriber::set_default(Collector(captured.clone()));
    (captured, guard)
}

struct Collector(Captured);

struct Fields<'a>(&'a Captured);

impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        let _ = write!(self.0 .0.lock().unwrap(), "{}={value:?} ", field.name());
    }
}

impl Subscriber for Collector {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }

    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::TRACE)
    }

    fn new_span(&self, attributes: &span::Attributes<'_>) -> span::Id {
        attributes.record(&mut Fields(&self.0));
        span::Id::from_u64(1)
    }

    fn record(&self, _: &span::Id, values: &span::Record<'_>) {
        values.record(&mut Fields(&self.0));
    }

    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}

    fn event(&self, event: &Event<'_>) {
        let _ = write!(self.0 .0.lock().unwrap(), "{} ", event.metadata().name());
        event.record(&mut Fields(&self.0));
    }

    fn enter(&self, _: &span::Id) {}

    fn exit(&self, _: &span::Id) {}
}
