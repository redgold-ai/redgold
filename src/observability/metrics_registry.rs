//! This example is part unit test and part demonstration.
//!
//! We show all of the registration macros, as well as all of the "emission" macros, the ones you
//! would actually call to update a metric.
//!
//! We demonstrate the various permutations of values that can be passed in the macro calls, all of
//! which are documented in detail for the respective macro.

use log::info;
use metrics::{counter, describe_counter, describe_gauge, describe_histogram, KeyName, SharedString};
use metrics::{Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, Recorder, Unit};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::sync::Arc;

pub fn register_metric_names() {
    describe_counter!("redgold.p2p.request_peer_info", "");

    describe_counter!("redgold.node.main_started", "");
    describe_counter!("redgold.node.node_started", "");
    describe_counter!("redgold.node.async_started", "");

    describe_counter!("redgold.observation.created", "");
    describe_counter!("redgold.observation.received", "");
    describe_counter!("redgold.observation.insert", "");
    describe_counter!("redgold.observation.metadata.added", "");
    describe_counter!("redgold.observation.attempt", "");
    describe_counter!("redgold.observation.metadata.total", "");
    describe_counter!("redgold.observation.buffer.added", "");
    describe_counter!("redgold.observation.failed_to_send_to_transaction_processor", "");
    describe_gauge!("redgold.observation.height", "");
    describe_gauge!("redgold.observation.total", "");
    describe_gauge!("redgold.observation.last.size", "");
    describe_gauge!("redgold.utxo.total", "");

    describe_counter!("redgold.transaction.accepted", "");
    describe_gauge!("redgold.transaction.accepted.total", "");
    describe_counter!("redgold.transaction.received", "");
    describe_counter!("redgold.transaction.missing_response_channel", "");
    describe_counter!("redgold.transaction.resolve.input", "");
    describe_counter!("redgold.transaction.resolve.output", "");
    describe_counter!("redgold.transaction.resolve.input.errors", "");
    describe_counter!("redgold.transaction.resolve.output.errors", "");
    describe_gauge!("redgold.transaction.total", "");
    describe_histogram!("redgold.transaction.size_bytes", "");
    describe_histogram!("redgold.transaction.floating_inputs", "");
    describe_histogram!("redgold.transaction.total_output_amount", "");
    describe_histogram!("redgold.transaction.num_inputs", "");
    describe_histogram!("redgold.transaction.num_outputs", "");

    describe_counter!("redgold.multiparty.received", "");

    describe_counter!("redgold.datastore.utxo.insert", "");

    describe_counter!("redgold.api.control.num_requests", "");
    describe_counter!("redgold.blocks.created", "");
    describe_counter!("redgold.api.rosetta.account_balance", "");
    describe_counter!("redgold.api.rosetta.account_coins", "");

    describe_gauge!("redgold.e2e.num_peers", "");
    describe_counter!("redgold.e2e.failure", "");
    describe_counter!("redgold.e2e.success", "");
    describe_counter!("redgold.peer.message.received", "");
    describe_counter!("redgold.peer.rest.send.error", "");
    describe_counter!("redgold.peer.rest.send", "");
    describe_counter!("redgold.peer.send", "");
    describe_counter!("redgold.peer.discovery.recv_for_each", "");
    describe_counter!("redgold.peer.rest.send.timeout", "");

    describe_counter!("redgold.recent_download.resolve_input_error", "");

    // describe_gauge!("redgold.libp2p.active_connections", "");
    // describe_counter!("redgold.libp2p.total_established_connections", "");
    // describe_counter!("redgold.libp2p.inbound_request", "");


}

struct PrintHandle(Key);

impl CounterFn for PrintHandle {
    fn increment(&self, value: u64) {
        println!("counter increment for '{}': {}", self.0, value);
    }

    fn absolute(&self, value: u64) {
        println!("counter absolute for '{}': {}", self.0, value);
    }
}

impl GaugeFn for PrintHandle {
    fn increment(&self, value: f64) {
        println!("gauge increment for '{}': {}", self.0, value);
    }

    fn decrement(&self, value: f64) {
        println!("gauge decrement for '{}': {}", self.0, value);
    }

    fn set(&self, value: f64) {
        println!("gauge set for '{}': {}", self.0, value);
    }
}

impl HistogramFn for PrintHandle {
    fn record(&self, value: f64) {
        println!("histogram record for '{}': {}", self.0, value);
    }
}
//
// #[derive(Default)]
// struct PrintRecorder;
//
// impl Recorder for PrintRecorder {
//     fn describe_counter(&self, key_name: KeyName, unit: Option<Unit>, description: &'static str) {
//         println!(
//             "(counter) registered key {} with unit {:?} and description {:?}",
//             key_name.as_str(),
//             unit,
//             description
//         );
//     }
//
//     fn describe_gauge(&self, key_name: KeyName, unit: Option<Unit>, description: &'static str) {
//         println!(
//             "(gauge) registered key {} with unit {:?} and description {:?}",
//             key_name.as_str(),
//             unit,
//             description
//         );
//     }
//
//     fn describe_histogram(&self, key_name: KeyName, unit: Option<Unit>, description: &'static str) {
//         println!(
//             "(histogram) registered key {} with unit {:?} and description {:?}",
//             key_name.as_str(),
//             unit,
//             description
//         );
//     }
//
//     fn register_counter(&self) -> Counter {
//         Counter::from_arc(Arc::new(PrintHandle(key.clone())))
//     }
//
//     fn register_gauge(&self, key: &Key) -> Gauge {
//         Gauge::from_arc(Arc::new(PrintHandle(key.clone())))
//     }
//
//     fn register_histogram(&self, key: &Key) -> Histogram {
//         Histogram::from_arc(Arc::new(PrintHandle(key.clone())))
//     }
// }
//
// pub fn init_print_logger() {
//     let recorder = PrintRecorder::default();
//     metrics::set_boxed_recorder(Box::new(recorder)).unwrap()
// }
pub fn init_prometheus(port_offset: u16) {
    let builder = PrometheusBuilder::new();
    use std::net::{Ipv4Addr, SocketAddrV4};
    let socket = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), port_offset - 1);
    let socket_fallback = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), port_offset - 2);
    // Normally, most users will want to "install" the exporter which sets it as the
    // global recorder for all `metrics` calls, and installs either an HTTP listener
    // when running as a scrape endpoint, or a simple asynchronous task which pushes
    // to the configured push gateway on the given interval.
    //
    // If you're already inside a Tokio runtime, this will spawn a task for the
    // exporter on that runtime, and otherwise, a new background thread will be
    // spawned which a Tokio single-threaded runtime is launched on to, where we then
    // finally launch the exporter:
    let err = builder.with_http_listener(socket).install();
    // TODO: Change the port here by first parsing args associated with metrics / logs
    if err.is_err() {
        info!(
            "Failed to install Prometheus exporter, falling back to {:?}",
            socket_fallback
        );
        let builder2 = PrometheusBuilder::new();
        builder2
            .with_http_listener(socket_fallback)
            .install()
            .expect("failed to install recorder/exporter on fallback socket");
    }
}

enum MetricType {
    Counter,
    Gauge,
    Histogram,
}

pub fn register_metrics(port_offset: u16) {
    if std::env::var("REDGOLD_LOCAL_DEBUG").is_ok() {
        // init_print_logger();
    } else {
        init_prometheus(port_offset);
    }
    register_metric_names();
}

// let _common_labels = &[("listener", "frontend")];

// // Go through describing the metrics:
// describe_counter!("requests_processed", "number of requests processed");
// describe_counter!("bytes_sent", Unit::Bytes, "total number of bytes sent");
// describe_gauge!("connection_count", "current number of client connections");
// describe_histogram!(
//     "svc.execution_time",
//     Unit::Milliseconds,
//     "execution time of request handler"
// );
// describe_gauge!("unused_gauge", "some gauge we'll never use in this program");
// describe_histogram!(
//     "unused_histogram",
//     Unit::Seconds,
//     "some histogram we'll also never use in this program"
// );
//
// // And registering them:
// let counter1 = describe_counter!("test_counter", "");
// counter1.increment(1);
// let counter2 = describe_counter!("test_counter", "type" => "absolute", "");
// counter2.absolute(42);
//
// let gauge1 = describe_gauge!("test_gauge", "");
// gauge1.increment(1.0);
// let gauge2 = describe_gauge!("test_gauge", "type" => "decrement", "");
// gauge2.decrement(1.0);
// let gauge3 = describe_gauge!("test_gauge", "type" => "set", "");
// gauge3.set(3.1459);
//
// let histogram1 = describe_histogram!("test_histogram", "");
// histogram1.record(0.57721);
//
// // All the supported permutations of `counter!` and its increment/absolute versions:
// counter!("bytes_sent", 64);
// counter!("bytes_sent", 64, "listener" => "frontend");
// counter!("bytes_sent", 64, "listener" => "frontend", "server" => server_name.clone());
// counter!("bytes_sent", 64, common_labels);
//
// counter!("requests_processed").increment(1);
// counter!("requests_processed", "request_type" => "admin").increment(1);
// counter!("requests_processed", "request_type" => "admin", "server" => server_name.clone()).increment(1);
// counter!("requests_processed", common_labels).increment(1);
//
// absolute_counter!("bytes_sent", 64);
// absolute_counter!("bytes_sent", 64, "listener" => "frontend");
// absolute_counter!("bytes_sent", 64, "listener" => "frontend", "server" => server_name.clone());
// absolute_counter!("bytes_sent", 64, common_labels);
//
// // All the supported permutations of `gauge!` and its increment/decrement versions:
// gauge!("connection_count", 300.0);
// gauge!("connection_count", 300.0, "listener" => "frontend");
// gauge!("connection_count", 300.0, "listener" => "frontend", "server" => server_name.clone());
// gauge!("connection_count", 300.0, common_labels);
// increment_gauge!("connection_count", 300.0);
// increment_gauge!("connection_count", 300.0, "listener" => "frontend");
// increment_gauge!("connection_count", 300.0, "listener" => "frontend", "server" => server_name.clone());
// increment_gauge!("connection_count", 300.0, common_labels);
// decrement_gauge!("connection_count", 300.0);
// decrement_gauge!("connection_count", 300.0, "listener" => "frontend");
// decrement_gauge!("connection_count", 300.0, "listener" => "frontend", "server" => server_name.clone());
// decrement_gauge!("connection_count", 300.0, common_labels);
//
// // All the supported permutations of `histogram!`:
// histogram!("svc.execution_time", 70.0);
// histogram!("svc.execution_time", 70.0, "type" => "users");
// histogram!("svc.execution_time", 70.0, "type" => "users", "server" => server_name.clone());
// histogram!("svc.execution_time", 70.0, common_labels);
// describe_counter!("bytes_sent", Unit::Bytes, "");
// describe_gauge!("connection_count", common_labels, "");
// register_histogram!(
//     "svc.execution_time",
//     Unit::Milliseconds,
//     "execution time of request handler"
// );
// describe_gauge!("unused_gauge", "service" => "backend", "");
// describe_histogram!("unused_histogram", Unit::Seconds, "unused histo", "service" => "middleware", "");
//
// // All the supported permutations of `increment!`:
// counter!("requests_processed").increment(1);
// counter!("requests_processed", "request_type" => "admin").increment(1);
// counter!("requests_processed", "request_type" => "admin", "server" => server_name.clone()).increment(1);
// counter!("requests_processed", common_labels).increment(1);
//
// // All the supported permutations of `counter!`:
// counter!("bytes_sent", 64);
// counter!("bytes_sent", 64, "listener" => "frontend");
// counter!("bytes_sent", 64, "listener" => "frontend", "server" => server_name.clone());
// counter!("bytes_sent", 64, common_labels);
//
// // All the supported permutations of `gauge!` and its increment/decrement versions:
// gauge!("connection_count", 300.0);
// gauge!("connection_count", 300.0, "listener" => "frontend");
// gauge!("connection_count", 300.0, "listener" => "frontend", "server" => server_name.clone());
// gauge!("connection_count", 300.0, common_labels);
// increment_gauge!("connection_count", 300.0);
// increment_gauge!("connection_count", 300.0, "listener" => "frontend");
// increment_gauge!("connection_count", 300.0, "listener" => "frontend", "server" => server_name.clone());
// increment_gauge!("connection_count", 300.0, common_labels);
// decrement_gauge!("connection_count", 300.0);
// decrement_gauge!("connection_count", 300.0, "listener" => "frontend");
// decrement_gauge!("connection_count", 300.0, "listener" => "frontend", "server" => server_name.clone());
// decrement_gauge!("connection_count", 300.0, common_labels);
//
// // All the supported permutations of `histogram!`:
// histogram!("svc.execution_time", 70.0);
// histogram!("svc.execution_time", 70.0, "type" => "users");
// histogram!("svc.execution_time", 70.0, "type" => "users", "server" => server_name.clone());
// histogram!("svc.execution_time", 70.0, common_labels);
// }
