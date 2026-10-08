//! The demo plugin (see [`kineto_plugins::demo`]).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    kineto_plugins::demo::serve(&args);
}
