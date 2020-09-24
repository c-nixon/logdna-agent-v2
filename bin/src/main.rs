#[macro_use]
extern crate log;

use std::path::PathBuf;
use std::thread::spawn;

use async_trait::async_trait;
use config::Config;
use http::client::Client;
#[cfg(use_systemd)]
use journald::source::JournaldSource;
use k8s::middleware::K8sMetadata;
use metrics::Metrics;
use middleware::Executor;
use source::{SourceCollection, Source};
use std::cell::RefCell;
use std::rc::Rc;
use http::types::body::LineBuilder;
use tokio::sync::mpsc::Sender;
use fs::tail::Tailer;

#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

/*
#[cfg(use_systemd)]
fn register_journald_source(source_reader: &mut SourceReader) {
    source_reader.register(JournaldSource::new());
}

#[cfg(not(use_systemd))]
fn register_journald_source(_source_reader: &mut SourceReader) {}
*/

enum AgentSources {
    Tailer(Tailer),
}

#[async_trait]
impl Source for AgentSources {
    async fn begin_processing(&mut self, sender: Sender<Vec<LineBuilder>>) {
        match self {
            AgentSources::Tailer(tailer) => tailer.begin_processing(sender),
        };
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();
    info!("running version: {}", env!("CARGO_PKG_VERSION"));

    let config = match Config::new() {
        Ok(v) => v,
        Err(e) => {
            error!("config error: {}", e);
            std::process::exit(1);
        }
    };

    spawn(Metrics::start);

    let client = Rc::new(RefCell::new(Client::new(config.http.template)));
    client
        .borrow_mut()
        .set_max_buffer_size(config.http.body_size);
    client.borrow_mut().set_timeout(config.http.timeout);

    let mut executor = Executor::new();
    if PathBuf::from("/var/log/containers/").exists() {
        match K8sMetadata::new() {
            Ok(v) => executor.register(v),
            Err(e) => warn!("{}", e),
        };
    }

    let mut source_collection = SourceCollection::<AgentSources>::new();
    source_collection.register(AgentSources::Tailer(Tailer::new(config.log.dirs, config.log.rules))).await;

    executor.init();

    /*
    loop {
        source_reader.drain(Box::new(|lines| {
            if let Some(lines) = executor.process(lines) {
                for line in lines {
                    client.borrow_mut().send(line)
                }
            }
        }));
        client.borrow_mut().poll();
        sleep(SLEEP_DURATION);
    }
    */
}
