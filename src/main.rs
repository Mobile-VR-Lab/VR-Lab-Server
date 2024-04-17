use std::sync::Arc;

use clap::Parser;
use server::initialize_server;
use tokio::sync::RwLock;

mod server;
mod models;
mod api;

// Type shared across workers.
// Note: It is EXPENSIVE to copy these pointers. It is an atomic reference-counting pointer, and atomic operations are slow.
pub type AsyncHandle<T> = Arc<RwLock<T>>;

/*
Add command line arguments here. See the clap documentation for details on how this works.
*/
#[derive(clap::Parser)]
#[command(version, about)]
struct Args {
    #[arg(short, long, default_value_t = ("0.0.0.0:15300").into())]
    address: String,
    #[arg(short, long, default_value_t = ("0.0.0.0:8080").into())]
    web_address: String,
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let args = Args::parse();
    log::info!("VR Lab Server starting...");

    let (headset_server, ch) = initialize_server(&args.address).await;
    //let api_result 
    let _ = 
        api::init_api(args.web_address.as_str(), headset_server.clone()).await;

    log::info!("Done!");

    // We may want to implement something here for development/debugging. Server console?
    // There are easy-to-use CLI builders for Rust, which could be put to use.

    //api_result.await.unwrap().unwrap();
    ch.await.ok();
    log::warn!("Connection thread died. Server shutting down...");
}
