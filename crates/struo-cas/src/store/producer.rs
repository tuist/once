use super::{stream, Cas};
use crate::{Digest, Result};

impl Cas {
    pub(crate) async fn put_produced<W, G>(&self, guard: G, produce: W) -> Result<Digest>
    where
        W: FnOnce(&mut dyn std::io::Write) -> std::io::Result<()> + Send + 'static,
        G: Send + 'static,
    {
        stream::produce(self, guard, produce, false).await
    }

    pub(crate) async fn put_produced_pipelined<W, G>(&self, guard: G, produce: W) -> Result<Digest>
    where
        W: FnOnce(&mut dyn std::io::Write) -> std::io::Result<()> + Send + 'static,
        G: Send + 'static,
    {
        stream::produce(self, guard, produce, true).await
    }
}

#[cfg(test)]
#[path = "producer_tests.rs"]
mod tests;
