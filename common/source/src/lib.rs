use async_trait::async_trait;
use http::types::body::LineBuilder;
use tokio::sync::mpsc::{channel, Receiver, Sender};

#[async_trait]
pub trait Source {
    async fn begin_processing(&mut self, sender: Sender<Vec<LineBuilder>>);
}

pub struct SourceCollection<T: Source> {
    source_sender: Sender<Vec<LineBuilder>>,
    source_receiver: Option<Receiver<Vec<LineBuilder>>>,
    sources: Vec<T>,
}

impl<T: Source> SourceCollection<T> {
    pub fn new() -> Self {
        let (source_sender, source_receiver) = channel(5000);
        Self {
            source_sender,
            source_receiver: Some(source_receiver),
            sources: Vec::new(),
        }
    }

    pub fn take_receiver(&mut self) -> Option<Receiver<Vec<LineBuilder>>> {
        self.source_receiver.take()
    }

    pub async fn register(&mut self, mut source: T) {
        source.begin_processing(self.source_sender.clone()).await;
        self.sources.push(source);
    }
}
