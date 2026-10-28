//! Runs the Wi‑Fi transport against a tiny fake of the firmware's HTTP + WebSocket servers.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use tungstenite::Message;
use x4_device::{wifi, Error, Transport};

type Files = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

fn http_server(files: Files) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in l.incoming() {
            let mut s = stream.unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();
            let target = first.split_whitespace().nth(1).unwrap().to_string();
            let (code, out) = if target.starts_with("/api/status") {
                (200, r#"{"version":"1.6.1","ip":"127.0.0.1","mode":"STA","rssi":-40,"freeHeap":80000,"uptime":5,"device":"X4","serial":"x"}"#.to_string())
            } else if target.starts_with("/api/files") {
                let f = files.lock().unwrap();
                let items: Vec<String> = f
                    .iter()
                    .map(|(n, d)| {
                        format!(
                            r#"{{"name":"{}","size":{},"isDirectory":false,"isEpub":true}}"#,
                            n.trim_start_matches('/'),
                            d.len()
                        )
                    })
                    .collect();
                (200, format!("[{}]", items.join(",")))
            } else if target.starts_with("/delete") {
                let path = body.strip_prefix("path=").unwrap().replace("%2F", "/");
                let mut f = files.lock().unwrap();
                let before = f.len();
                f.retain(|(n, _)| *n != path);
                if f.len() < before {
                    (200, "Deleted".into())
                } else {
                    (404, "Not found".into())
                }
            } else {
                (404, "nope".into())
            };
            let _ = write!(
                s,
                "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                out.len()
            );
        }
    });
    port
}

fn ws_server(files: Files) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in l.incoming() {
            let mut ws = tungstenite::accept(stream.unwrap()).unwrap();
            let Message::Text(start) = ws.read().unwrap() else {
                panic!()
            };
            let parts: Vec<&str> = start.split(':').collect();
            let (name, size, dir) = (
                parts[1].to_string(),
                parts[2].parse::<usize>().unwrap(),
                parts[3].to_string(),
            );
            let path = format!("{}/{}", dir.trim_end_matches('/'), name);
            if files.lock().unwrap().iter().any(|(n, _)| *n == path) {
                ws.send(Message::text(format!("ERROR:File already exists: {name}")))
                    .unwrap();
                continue;
            }
            ws.send(Message::text("READY")).unwrap();
            let mut data = Vec::new();
            while data.len() < size {
                if let Message::Binary(b) = ws.read().unwrap() {
                    data.extend_from_slice(&b);
                    ws.send(Message::text(format!("PROGRESS:{}:{size}", data.len())))
                        .unwrap();
                }
            }
            files.lock().unwrap().push((path, data));
            ws.send(Message::text("DONE")).unwrap();
            let _ = ws.read();
        }
    });
    port
}

#[test]
fn status_list_upload_overwrite() {
    let files: Files = Arc::new(Mutex::new(Vec::new()));
    let http = http_server(files.clone());
    let ws = ws_server(files.clone());
    let mut t = wifi::open(&format!("127.0.0.1:{http}"), Some(ws)).unwrap();

    let s = t.status().unwrap();
    assert_eq!(
        (s.device.as_str(), s.firmware.as_str(), s.free_heap),
        ("X4", "1.6.1", Some(80000))
    );

    let data: Vec<u8> = (0..20_000).map(|i| i as u8).collect();
    let mut last = (0, 0);
    t.write("/book.epub", &data, false, &mut |d, n| last = (d, n))
        .unwrap();
    assert_eq!(last, (20_000, 20_000));
    assert_eq!(files.lock().unwrap()[0].1, data);

    let err = t
        .write("/book.epub", b"new", false, &mut |_, _| {})
        .unwrap_err();
    assert!(
        matches!(err, Error::Device(ref c) if c == "target_exists"),
        "{err}"
    );

    t.write("/book.epub", b"new", true, &mut |_, _| {}).unwrap();
    let list = t.list("/").unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].path.as_str(), list[0].size), ("/book.epub", 3));
}
