//! Quick WebSocket connection test

use std::net::{SocketAddr, ToSocketAddrs};
use tokio::net::TcpStream;
use tokio_tungstenite::connect_async;

#[tokio::main]
async fn main() {
    println!("Starting WebSocket test...");

    // Resolve DNS and try IPv4 specifically
    println!("Resolving DNS...");
    let addrs: Vec<SocketAddr> = "ws-subscriptions-clob.polymarket.com:443"
        .to_socket_addrs()
        .unwrap()
        .collect();

    println!("Resolved addresses: {:?}", addrs);

    // Find an IPv4 address
    let ipv4_addr = addrs.iter().find(|a| a.is_ipv4());
    let ipv6_addr = addrs.iter().find(|a| a.is_ipv6());

    if let Some(addr) = ipv4_addr {
        println!("Testing TCP to IPv4: {}", addr);
        match tokio::time::timeout(std::time::Duration::from_secs(5), TcpStream::connect(addr))
            .await
        {
            Ok(Ok(_)) => println!("IPv4 TCP succeeded!"),
            Ok(Err(e)) => println!("IPv4 TCP failed: {:?}", e),
            Err(_) => println!("IPv4 TCP timed out!"),
        }
    }

    if let Some(addr) = ipv6_addr {
        println!("Testing TCP to IPv6: {}", addr);
        match tokio::time::timeout(std::time::Duration::from_secs(5), TcpStream::connect(addr))
            .await
        {
            Ok(Ok(_)) => println!("IPv6 TCP succeeded!"),
            Ok(Err(e)) => println!("IPv6 TCP failed: {:?}", e),
            Err(_) => println!("IPv6 TCP timed out!"),
        }
    }

    // Now test WebSocket
    let url = "wss://ws-subscriptions-clob.polymarket.com/ws/market";
    println!("\nTesting WebSocket connection to: {}", url);

    match tokio::time::timeout(std::time::Duration::from_secs(10), connect_async(url)).await {
        Ok(Ok((_ws, response))) => {
            println!("WebSocket connected! Status: {:?}", response.status());
        }
        Ok(Err(e)) => {
            println!("WebSocket error: {:?}", e);
        }
        Err(_) => {
            println!("WebSocket connection timed out!");
        }
    }
}
