use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::net::UdpSocket;
use tokio::sync::{Mutex, RwLock, RwLockReadGuard};

use crate::config::{Config, LogLevel};
use crate::config::LogLevel::*;
use crate::datagram::*;
use crate::server_lib::LogSource::*;
use crate::topic::TopicV2;

pub enum LogSource {
    DatagramsHandler,
    PingSender,
    DataHandler,
    HeartbeatChecker,
    TopicHandler,
    Other,
}

pub fn log(
    log_level: LogLevel,
    log_source: LogSource,
    message: String,
    config: Arc<Config>,
) {
    // If log level is under config log level do not show the message
    if log_level < config.debug_level { return; }

    match log_source {
        LogSource::DatagramsHandler => {
            if !config.debug_datagram_handler { return; }
            println!("[Server - DatagramHandler] {}: {}", display_loglevel(log_level), message);
        }
        LogSource::PingSender => {
            if !config.debug_ping_sender { return; }
            println!("[Server - PingSender] {}: {}", display_loglevel(log_level), message);
        }
        LogSource::DataHandler => {
            if !config.debug_data_handler { return; }
            println!("[Server - DataHandler] {}: {}", display_loglevel(log_level), message);
        }
        LogSource::HeartbeatChecker => {
            if !config.debug_heartbeat_checker { return; }
            println!("[Server - HeartbeatChecker] {}: {}", display_loglevel(log_level), message);
        }
        LogSource::TopicHandler => {
            if !config.debug_topic_handler { return; }
            println!("[Server - TopicHandler] {}: {}", display_loglevel(log_level), message);
        }
        LogSource::Other => {
            println!("[Server] {}", message);
        }
    }
}


/**
This method return true if the client was already connected, it return the old
client id too.
@param ip &IpAddr : ip address of the tested client
@param clients MutexGuard<HashMap<u64, SocketAddr>> : Hashmap containing all clients address and clients id.

@return (bool, u64)
 */
pub async fn already_connected<'a>(
    ip: &'a IpAddr,
    clients: RwLockReadGuard<'a, HashMap<u64, SocketAddr>>,
) -> (bool, u64) {
    for (key, value) in clients.iter() {
        if ip == &value.ip() {
            return (true, *key);
        }
    }
    return (false, 0);
}

/**
This methods return a uniq id for a new client
@param clients MutexGuard<HashMap<u64, SocketAddr>> : Hashmap containing all clients address and clients id.

@return u64
 */
pub fn get_new_id() -> u64 {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_nanos();

    let xor_key: u64 = rand::random();
    // return the XOR operation of the current time and the random number
    return (timestamp as u64) ^ xor_key;
}

/**
This methods return a uniq id for a new ping reference
@param pings MutexGuard<HashMap<u8, u128>> : Hashmap containing all ping id and the time reference.

@return u8
 */
fn get_new_ping_id() -> u8 {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_nanos();

    let xor_key: u8 = rand::random();
    // return the XOR operation of the current time and the random number
    return (timestamp as u8) ^ xor_key;
}

/**
This methods return the id of the given client
@param src &SocketAddr : The tested client
@param clients MutexGuard<HashMap<u64, SocketAddr>> : Hashmap containing all clients address and clients id.

@return Option<u64>
 */
pub async fn get_client_id(
    src: &SocketAddr,
    clients: Arc<RwLock<HashMap<u64, SocketAddr>>>,
) -> Option<u64> {
    for (key, val) in clients.read().await.iter() {
        if val == src {
            return Some(*key);
        }
    }
    None
}

/**
This methods handle the connexion of a new client to the server.
@param src SocketAddr : The new client
@param clients MutexGuard<HashMap<u64, SocketAddr>> : Hashmap containing all clients address and clients id.
@param socket Arc<UdpSocket> : Server socket to exchange datagrams with clients

@return none
 */
pub async fn handle_connect(
    src: SocketAddr,
    clients: Arc<RwLock<HashMap<u64, SocketAddr>>>,
    socket: Arc<UdpSocket>,
    config: Arc<Config>,
) -> bool {
    let (is_connected, current_id) = already_connected(&src.ip(), clients.read().await).await;
    let uuid;
    let result;
    if is_connected {
        uuid = current_id;
        log(Info, DatagramsHandler, format!("{} was already a client, UUID : {}", src.ip(), uuid), config.clone());
    } else {
        uuid = get_new_id();
        log(Info, DatagramsHandler, format!("{} is now a client, UUID : {}", src.ip(), uuid), config.clone());
        let mut map = clients.write().await;
        map.insert(uuid, src);
    }
    let datagram = &RQ_Connect_ACK_OK::new(uuid, config.heart_beat_period).as_bytes();
    result = socket.send_to(datagram, src).await;
    match result {
        Ok(bytes) => {
            log(Info, DatagramsHandler, format!("Send {} bytes (RQ_Connect_ACK_OK) to {}", bytes, src.ip()), config.clone());
        }
        Err(error) => {
            log(Error, DatagramsHandler, format!("Failed to send Connect ACK to {}.\nError: {}", src.ip(), error), config.clone());
        }
    }
    return is_connected;
}

pub async fn create_topics(path: &str, root: Arc<RwLock<TopicV2>>) -> Result<u64, String> {
    TopicV2::create_topics_gpt(path, root).await
}


pub async fn get_new_ping_reference(pings: Arc<Mutex<HashMap<u8, u128>>>, config: Arc<Config>) -> u8 {
    let key = get_new_ping_id();

    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis(); // Current time in ms

    pings.lock().await.insert(key, time);
    log(Info, PingSender, format!("New ping reference created. Id : {}", key), config.clone());
    return key;
}

/**
This method check if heartbeat are sent correctly and else close the client session
@param client_id u64 : The client identifier.
TODO : completer la doc
@param clients Arc<RwLock<HashMap<u64, SocketAddr>>> : An atomic reference of the pings HashMap. The map is protected by a rwlock to be thread safe

@return None
 */
pub async fn handle_pong(
    client_id: u64,
    ping_id: u8,
    current_time: u128,
    pings_ref: Arc<Mutex<HashMap<u8, u128>>>,
    clients_ping: Arc<RwLock<HashMap<u64, u128>>>,
    clients: Arc<RwLock<HashMap<u64, SocketAddr>>>,
    config: Arc<Config>,
) {
    // 0 - if client are not in the client array, they are offline so abort the treatment
    if !clients.read().await.contains_key(&client_id) { return; };

    // 1 - get the mutable ref of all ping request
    let mut pings_ref_mut = pings_ref.lock().await;
    // 2 - compute the round trip
    let round_trip = (current_time - pings_ref_mut.get(&ping_id).unwrap()) / 2;
    // 3 - free the ping_id
    pings_ref_mut.remove(&ping_id);
    // 4 - set the ping for the client_id
    clients_ping.write().await.entry(client_id)
        .and_modify(|v| *v = round_trip)
        .or_insert(round_trip);
    log(Info, PingSender, format!("There is {}ms of ping between {} and the server", round_trip, client_id), config.clone());
}


pub async fn is_online(
    client_id: u64,
    clients: Arc<RwLock<HashMap<u64, SocketAddr>>>,
) -> bool
{
    let clients_read = clients.read().await;
    return clients_read.contains_key(&client_id);
}

const FNV_PRIME: u64 = 1099511628211;
const FNV_OFFSET: u64 = 14695981039346656037;

pub fn custom_string_hash(s: &str) -> u64 {
    let mut hash = FNV_OFFSET;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}