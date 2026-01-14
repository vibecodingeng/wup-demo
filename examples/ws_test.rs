//! Quick WebSocket connection test

use tokio_tungstenite::connect_async;

#[tokio::main]
async fn main() {
    println!("Starting WebSocket test...");
    
    let url = "wss://ws-subscriptions-clob.polymarket.com/ws/market";
    println!("Connecting to: {}", url);
    
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        connect_async(url)
    ).await {
        Ok(Ok((ws, response))) => {
            println!("Connected! Status: {:?}", response.status());
        }
        Ok(Err(e)) => {
            println!("Connection error: {:?}", e);
        }
        Err(_) => {
            println!("Connection timed out after 10 seconds!");
        }
    }
}

