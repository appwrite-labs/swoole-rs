//! Upstream's `core-tests/src/protocol/redis.cpp` (`redis.get`, `redis.server`) and
//! `core-tests/src/server/mqtt.cpp` (`mqtt.echo`) TESTs, one function per TEST with the
//! same inputs and expected values. Built into the http server case binary
//! (tests/modules/http/server), whose http1/case.rs builds the Servers. The redis client is
//! a blocking `TcpStream` speaking RESP with core-tests' `RedisClient` (hiredis) reply
//! rules; upstream's runs in a coroutine, here in the worker's case thread. The file's
//! other TESTs are unit tests: `format` and `parse` in crates/protocols-ffi/src/redis.rs,
//! `recv_packet_with_partially_filled_buffer` in crates/ports/src/resp_recv.rs.

use super::protocol::random_bytes;
use super::{
    SIGTERM, Server, TEST_LOG_FILE, base_worker_start, exec_js_script, info, kill, primary_port,
    request, send, signal_block_all, swr_case_close, swr_case_master_pid, swr_case_mqtt_server,
    swr_case_redis_server, swr_case_tick,
};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_long, c_void};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::Mutex;

const REDIS_TEST_KEY: &str = "key-swoole";
const REDIS_TEST_VALUE: &str = "value-swoole";

unsafe extern "C" {
    fn time(out: *mut i64) -> i64;
    fn localtime_r(time: *const i64, tm: *mut c_void) -> *mut c_void;
    fn strftime(out: *mut c_char, size: usize, format: *const c_char, tm: *const c_void) -> usize;
}

/// A hiredis reply: its type byte, `str` (status, error and bulk replies) and `integer`.
struct Reply {
    kind: u8,
    str: Option<Vec<u8>>,
    integer: i64,
}

/// core-tests' `RedisClient` (`_lib/redis.cpp`) over a blocking socket.
struct RedisClient(BufReader<TcpStream>);

impl RedisClient {
    fn connect(host: &str, port: c_int) -> Option<Self> {
        TcpStream::connect((host, u16::try_from(port).ok()?))
            .ok()
            .map(|stream| Self(BufReader::new(stream)))
    }

    fn line(&mut self) -> Vec<u8> {
        let mut line = Vec::new();
        self.0.read_until(b'\n', &mut line).unwrap();
        assert!(line.ends_with(b"\r\n"), "a RESP line");
        line.truncate(line.len() - 2);
        line
    }

    /// `redisCommandArgv`: an array of bulk strings, then one reply.
    fn request(&mut self, args: &[&[u8]]) -> Reply {
        let mut wire = format!("*{}\r\n", args.len()).into_bytes();
        for arg in args {
            wire.extend(format!("${}\r\n", arg.len()).bytes());
            wire.extend(*arg);
            wire.extend(b"\r\n");
        }
        self.0.get_mut().write_all(&wire).unwrap();
        let line = self.line();
        let (kind, rest) = (line[0], &line[1..]);
        let number = || std::str::from_utf8(rest).unwrap().parse::<i64>().unwrap();
        match kind {
            b'+' | b'-' => Reply {
                kind,
                str: Some(rest.to_vec()),
                integer: 0,
            },
            b':' => Reply {
                kind,
                str: None,
                integer: number(),
            },
            b'$' if number() < 0 => Reply {
                kind,
                str: None,
                integer: 0,
            },
            b'$' => {
                let mut data = vec![0; usize::try_from(number()).unwrap() + 2];
                std::io::Read::read_exact(&mut self.0, &mut data).unwrap();
                data.truncate(data.len() - 2);
                Reply {
                    kind,
                    str: Some(data),
                    integer: 0,
                }
            }
            _ => panic!("unexpected reply {line:?}"),
        }
    }

    /// `Set` and `Select`: a status reply starting `OK`.
    fn ok(reply: &Reply) -> bool {
        reply.kind == b'+' && reply.str.as_ref().is_some_and(|s| s.starts_with(b"OK"))
    }

    fn set(&mut self, key: &str, value: &[u8]) -> bool {
        Self::ok(&self.request(&[b"SET", key.as_bytes(), value]))
    }

    /// `Get` and `Role`: the reply's `str`, else empty.
    fn get(&mut self, key: &str) -> Vec<u8> {
        self.request(&[b"GET", key.as_bytes()])
            .str
            .unwrap_or_default()
    }

    fn ttl(&mut self, key: &str) -> i64 {
        self.request(&[b"TTL", key.as_bytes()]).integer
    }

    fn select(&mut self, db: i32) -> bool {
        Self::ok(&self.request(&[b"SELECT", db.to_string().as_bytes()]))
    }

    fn role(&mut self) -> Vec<u8> {
        self.request(&[b"ROLE"]).str.unwrap_or_default()
    }
}

/// `TEST(redis, get)`: against the Redis server on 6379.
fn get() {
    let mut redis = RedisClient::connect("127.0.0.1", 6379).expect("Connect");
    assert!(redis.set(REDIS_TEST_KEY, REDIS_TEST_VALUE.as_bytes()));
    assert_eq!(redis.get(REDIS_TEST_KEY), REDIS_TEST_VALUE.as_bytes());
}

/// `redis::parse` of a request: its bulk strings.
fn parse(mut data: &[u8]) -> Vec<Vec<u8>> {
    // A `*N` or `$N` line's N, and the rest.
    fn number(data: &[u8]) -> Option<(usize, &[u8])> {
        let end = data.windows(2).position(|w| w == b"\r\n")?;
        let n = std::str::from_utf8(&data[1..end]).ok()?.parse().ok()?;
        Some((n, &data[end + 2..]))
    }
    let Some((count, rest)) = number(data) else {
        return Vec::new();
    };
    data = rest;
    let mut list = Vec::new();
    for _ in 0..count {
        let Some((length, rest)) = number(data) else {
            break;
        };
        list.push(rest[..length].to_vec());
        data = &rest[length + 2..];
    }
    list
}

static REDIS_DATA: Mutex<Option<HashMap<Vec<u8>, Vec<u8>>>> = Mutex::new(None);

/// redis.server's `onReceive`: GET and SET over a map, TTL -1, ROLE `master`, else an error.
extern "C" fn redis_receive(serv: *mut Server, req: *const c_void) -> c_int {
    let (session_id, data, _) = request(req);
    let list = parse(data);
    let command = list[0].to_ascii_uppercase();
    let mut store = REDIS_DATA.lock().unwrap();
    let store = store.get_or_insert_default();
    let buffer: Vec<u8> = match command.as_slice() {
        b"GET" => match store.get(&list[1]) {
            None => b"$-1\r\n".to_vec(),
            Some(value) => {
                send(serv, session_id, format!("${}\r\n", value.len()).as_bytes());
                send(serv, session_id, value);
                send(serv, session_id, b"\r\n");
                return 0;
            }
        },
        b"SET" => {
            store.insert(list[1].clone(), list[2].clone());
            b"+OK\r\n".to_vec()
        }
        b"TTL" => b":-1\r\n".to_vec(),
        b"ROLE" => b"$6\r\nmaster\r\n".to_vec(),
        _ => b"-Not Suppport\r\n".to_vec(),
    };
    send(serv, session_id, &buffer);
    0
}

/// `TEST(redis, server)`.
fn server() {
    *REDIS_DATA.lock().unwrap() = None;
    // SAFETY: the builder's callbacks have the slots' signatures.
    let serv = unsafe { swr_case_redis_server(base_worker_start, redis_receive) };
    super::protocol::serve(serv, |serv| {
        signal_block_all();
        let mut redis = RedisClient::connect("127.0.0.1", primary_port(serv)).expect("Connect");
        assert!(redis.set(REDIS_TEST_KEY, REDIS_TEST_VALUE.as_bytes()));
        assert_eq!(redis.get(REDIS_TEST_KEY), REDIS_TEST_VALUE.as_bytes());
        assert_eq!(redis.get(&format!("{REDIS_TEST_KEY}-not-exists")), b"");
        let data = random_bytes(128 * 1024, true);
        let big = format!("{REDIS_TEST_KEY}-big-key");
        assert!(redis.set(&big, &data));
        assert_eq!(redis.get(&big), data);
        assert_eq!(redis.ttl(REDIS_TEST_KEY), -1);
        assert!(!redis.select(1));
        assert_eq!(redis.role(), b"master");
        // SAFETY: SIGTERM to the running server's master.
        unsafe { kill(swr_case_master_pid(serv), SIGTERM) };
    });
}

pub(super) const REDIS: super::Cases = &[("get", get), ("server", server)];

/// `MqttSession`.
#[derive(Default)]
struct MqttSession {
    subscribed: bool,
    count: u16,
    packet_id_subscribe: u16,
    subscribed_topic: Vec<u8>,
}

static SESSIONS: Mutex<Option<HashMap<c_long, MqttSession>>> = Mutex::new(None);

fn sessions<T>(f: impl FnOnce(&mut HashMap<c_long, MqttSession>) -> T) -> T {
    f(SESSIONS.lock().unwrap().get_or_insert_default())
}

/// `current_timestamp()`: local `%Y-%m-%d %H:%M:%S`.
fn current_timestamp() -> String {
    let (mut tm, mut buf) = ([0u64; 8], [0u8; 64]);
    // SAFETY: time(2), then a writable struct tm (56 bytes on glibc) and a 64-byte buffer.
    let n = unsafe {
        let t = time(std::ptr::null_mut());
        localtime_r(&t, (&raw mut tm).cast());
        strftime(
            buf.as_mut_ptr().cast(),
            buf.len(),
            c"%Y-%m-%d %H:%M:%S".as_ptr(),
            (&raw const tm).cast(),
        )
    };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// `send_publish`: QoS 0, no identifier, the remaining length as a variable-length integer.
fn publish(topic: &[u8], message: &[u8]) -> Vec<u8> {
    let mut remaining_length = 2 + topic.len() + message.len();
    let mut packet = vec![0x30];
    loop {
        let mut byte = (remaining_length % 128) as u8;
        remaining_length /= 128;
        if remaining_length > 0 {
            byte |= 0x80;
        }
        packet.push(byte);
        if remaining_length == 0 {
            break;
        }
    }
    let topic_len = u16::try_from(topic.len()).unwrap();
    packet.extend(topic_len.to_be_bytes());
    packet.extend(topic);
    packet.extend(message);
    packet
}

/// `MqttSession::process_packet`: whether it handled the packet, and its reply.
fn process_packet(session: &mut MqttSession, data: &[u8]) -> (bool, Option<Vec<u8>>) {
    let len = data.len();
    match data[0] >> 4 {
        // CONNECT: CONNACK, session present 0, return code 0.
        1 => {
            println!("CONNECT received");
            (true, Some(vec![0x20, 0x02, 0x00, 0x00]))
        }
        // SUBSCRIBE: the identifier and first topic, then SUBACK with QoS 0.
        8 => {
            println!("SUBSCRIBE received");
            if len < 5 {
                return (false, None);
            }
            let packet_id = u16::from_be_bytes([data[2], data[3]]);
            session.packet_id_subscribe = packet_id;
            let mut pos = 4;
            if pos + 2 > len {
                return (false, None);
            }
            let topic_len = usize::from(u16::from_be_bytes([data[pos], data[pos + 1]]));
            pos += 2;
            if pos + topic_len > len {
                return (false, None);
            }
            session.subscribed_topic = data[pos..pos + topic_len].to_vec();
            println!(
                "subscribed topic: {}",
                String::from_utf8_lossy(&session.subscribed_topic)
            );
            session.subscribed = true;
            let [high, low] = packet_id.to_be_bytes();
            (true, Some(vec![0x90, 0x03, high, low, 0x00]))
        }
        // PUBLISH: PUBACK for QoS 1.
        3 => {
            println!("PUBLISH received");
            let qos = (data[0] & 0x0F & 0x06) >> 1;
            let remaining_length = data[1];
            assert!(remaining_length > 2);
            let mut pos = 2;
            if pos + 2 > len {
                return (false, None);
            }
            let topic_len = usize::from(u16::from_be_bytes([data[pos], data[pos + 1]]));
            pos += 2;
            if pos + topic_len > len {
                return (false, None);
            }
            let topic = String::from_utf8_lossy(&data[pos..pos + topic_len]).into_owned();
            pos += topic_len;
            let mut packet_id = 0u16;
            if qos > 0 {
                if pos + 2 > len {
                    return (false, None);
                }
                packet_id = u16::from_be_bytes([data[pos], data[pos + 1]]);
                pos += 2;
            }
            if pos > len {
                return (false, None);
            }
            let payload = String::from_utf8_lossy(&data[pos..]);
            println!("topic: {topic}, payload: {payload}, QoS: {qos}");
            if qos == 1 {
                let [high, low] = packet_id.to_be_bytes();
                return (true, Some(vec![0x40, 0x02, high, low]));
            }
            (true, None)
        }
        packet_type => {
            println!("unhandled packet type: {packet_type}");
            (true, None)
        }
    }
}

/// `onConnect`'s timer: once subscribed, a PUBLISH every 100ms; after the eleventh,
/// DISCONNECT and close.
extern "C" fn mqtt_tick(serv: *mut Server, session_id: c_long) -> bool {
    let Some((topic, count)) = sessions(|s| {
        let session = s.get_mut(&session_id)?;
        if !session.subscribed {
            return Some((None, 0));
        }
        let count = session.count;
        session.count += 1;
        Some((Some(session.subscribed_topic.clone()), count))
    }) else {
        return false;
    };
    let Some(topic) = topic else {
        return true;
    };
    let message = format!("Index: {count}, Time: {}", current_timestamp());
    send(serv, session_id, &publish(&topic, message.as_bytes()));
    if count + 1 > 10 {
        send(serv, session_id, &[0xE0, 0x00]);
        // SAFETY: the server's own session; onClose runs with the sessions unlocked.
        unsafe { swr_case_close(serv, session_id) };
        return false;
    }
    true
}

extern "C" fn mqtt_connect(serv: *mut Server, head: *const c_void) {
    let (session_id, _, _, _) = info(head);
    sessions(|s| s.insert(session_id, MqttSession::default()));
    // SAFETY: the running server and a callback of the builder's signature.
    unsafe { swr_case_tick(serv, 100, mqtt_tick, session_id) };
}

extern "C" fn mqtt_close(_: *mut Server, head: *const c_void) {
    let (session_id, _, _, _) = info(head);
    sessions(|s| s.remove(&session_id));
}

extern "C" fn mqtt_receive(serv: *mut Server, req: *const c_void) -> c_int {
    let (session_id, data, _) = request(req);
    let (ok, reply) =
        sessions(|s| process_packet(s.get_mut(&session_id).expect("a session"), data));
    if let Some(reply) = reply {
        send(serv, session_id, &reply);
    }
    if !ok {
        eprintln!("failed to process the packet");
    }
    0
}

/// `TEST(mqtt, echo)`: core-tests/js/mqtt.js subscribes, then logs ten publishes.
fn echo() {
    *SESSIONS.lock().unwrap() = None;
    // SAFETY: the builder's callbacks have the slots' signatures.
    let serv =
        unsafe { swr_case_mqtt_server(base_worker_start, mqtt_connect, mqtt_close, mqtt_receive) };
    super::protocol::serve(serv, |serv| {
        signal_block_all();
        let status = exec_js_script("mqtt.js", &primary_port(serv).to_string());
        // SAFETY: SIGTERM to the running server's master.
        unsafe { kill(swr_case_master_pid(serv), SIGTERM) };
        assert_eq!(status, Some(0));
    });
    let str = std::fs::read_to_string(TEST_LOG_FILE).expect("the log file");
    for i in 0..10 {
        assert!(str.contains(&format!(
            "received message, topic: test/topic, content: Index: {i}, Time: "
        )));
    }
    let _ = std::fs::remove_file(TEST_LOG_FILE);
}

pub(super) const MQTT: super::Cases = &[("echo", echo)];
