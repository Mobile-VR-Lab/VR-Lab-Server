use std::sync::Arc;

use server::initialize_server;
use tokio::sync::RwLock;

mod server;
mod models;
mod api;

// Type shared across workers.
// Note: It is EXPENSIVE to copy these pointers. It is an atomic reference-counting pointer, and atomic operations are slow.
pub type AsyncHandle<T> = Arc<RwLock<T>>;

#[tokio::main]
async fn main() {
    env_logger::init();
    log::info!("VR Lab Server starting...");

    let (headset_server, _, ch) = initialize_server().await;
    //let api_result 
    let _ = 
        api::init_api(headset_server.clone()).await;

    log::info!("Done!");

    // We may want to implement something here for development/debugging. Server console?
    // There are easy-to-use CLI builders for Rust, which could be put to use.

    //api_result.await.unwrap().unwrap();
    ch.await.ok();
    log::warn!("Connection thread died. Server shutting down...");
}
