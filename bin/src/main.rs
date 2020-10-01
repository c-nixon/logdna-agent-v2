#[macro_use]
extern crate log;

use std::path::PathBuf;
use std::thread::spawn;

use async_trait::async_trait;
use config::Config;
use fs::tail::Tailer;
use http::client::Client;
use http::types::body::LineBuilder;
#[cfg(use_systemd)]
use journald::source::JournaldSource;
use k8s::middleware::K8sMetadata;
use metrics::Metrics;
use middleware::Executor;
use source::{Source, SourceCollection};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

static SLEEP_DURATION: Duration = Duration::from_millis(10);

// Statically include the CARGO_PKG_NAME and CARGO_PKG_VERSIONs in the binary
// and export under the PKG_NAME and PKG_VERSION symbols.
// These are used to identify the application and version, for example as part
// of the user agent string.
#[no_mangle]
pub static PKG_NAME: &str = env!("CARGO_PKG_NAME");
#[no_mangle]
pub static PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

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
    source_collection
        .register(AgentSources::Tailer(Tailer::new(
            config.log.dirs,
            config.log.rules,
        )))
        .await;

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
