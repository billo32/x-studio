//! Runs the USB transport against an in-memory model of the firmware's
//! `UsbSerialFileTransfer.cpp`, including log noise and BUSY keep-alives on the line.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use x4_device::usb::UsbTransport;
use x4_device::{write_in_batch, Error, Transport};

#[derive(Default)]
struct FakeFirmware {
    on_home: bool,
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
    input: Vec<u8>,
    output: VecDeque<u8>,
    writing: Option<Upload>,
    corrupt_next_crc: bool,
    /// After each completed write: ignore input for a while, then report "not on home"
    /// for a while more — like a reader busy refreshing after a new book arrives.
    settle_after_write: bool,
    busy_until: Option<Instant>,
    off_home_until: Option<Instant>,
}

struct Upload {
    path: String,
    size: usize,
    data: Vec<u8>,
    waiting_crc: bool,
}

impl FakeFirmware {
    fn new() -> Self {
        let mut f = FakeFirmware {
            on_home: true,
            ..Default::default()
        };
        f.dirs.insert("/".into());
        f
    }

    fn say(&mut self, s: &str) {
        self.output.extend(s.as_bytes());
    }

    fn log_noise(&mut self) {
        self.say("[1234] [DBG] [EPUB] cache warm\n");
    }

    fn take_path(&mut self, at: usize) -> Option<(String, usize)> {
        if self.input.len() < at + 2 {
            return None;
        }
        let len = u16::from_le_bytes([self.input[at], self.input[at + 1]]) as usize;
        if self.input.len() < at + 2 + len {
            return None;
        }
        let p = String::from_utf8(self.input[at + 2..at + 2 + len].to_vec()).unwrap();
        Some((p, at + 2 + len))
    }

    fn children(&self, dir: &str) -> Vec<(String, Option<usize>)> {
        let prefix = if dir == "/" {
            "/".to_string()
        } else {
            format!("{dir}/")
        };
        let direct = |p: &String| {
            p.starts_with(&prefix) && p.len() > prefix.len() && !p[prefix.len()..].contains('/')
        };
        let mut out: Vec<_> = self
            .dirs
            .iter()
            .filter(|p| direct(p))
            .map(|p| (p[prefix.len()..].to_string(), None))
            .collect();
        out.extend(
            self.files
                .iter()
                .filter(|(p, _)| direct(p))
                .map(|(p, d)| (p[prefix.len()..].to_string(), Some(d.len()))),
        );
        out
    }

    fn process(&mut self) {
        if self.busy_until.is_some_and(|t| Instant::now() < t) {
            return;
        }
        loop {
            if let Some(up) = self.writing.as_mut() {
                if !up.waiting_crc {
                    let want = (up.size - up.data.len()).min(256);
                    if want == 0 || self.input.len() < want {
                        if want != 0 {
                            return;
                        }
                    } else {
                        let chunk: Vec<u8> = self.input.drain(..want).collect();
                        up.data.extend(chunk);
                        if up.data.len() < up.size {
                            self.output.push_back(0x06);
                            continue;
                        }
                    }
                    let up = self.writing.as_mut().unwrap();
                    up.waiting_crc = true;
                    self.say("BUSY:write:9999\n");
                    if self.writing.as_ref().unwrap().size > 0 {
                        self.output.push_back(0x06);
                    }
                    continue;
                }
                if self.input.len() < 4 {
                    return;
                }
                let crc = u32::from_le_bytes(
                    self.input
                        .drain(..4)
                        .collect::<Vec<_>>()
                        .try_into()
                        .unwrap(),
                );
                let up = self.writing.take().unwrap();
                if crc != crc32fast::hash(&up.data) {
                    self.say("ERR:crc:expected=0,actual=1,bytes=0\n");
                } else {
                    self.files.insert(up.path, up.data);
                    self.say("OK\n");
                    if self.settle_after_write {
                        let now = Instant::now();
                        self.busy_until = Some(now + Duration::from_millis(1200));
                        self.off_home_until = Some(now + Duration::from_millis(2000));
                        return;
                    }
                }
                continue;
            }

            let Some(start) = self.input.windows(4).position(|w| w == b"CMND") else {
                return;
            };
            if self.input.len() < start + 5 {
                return;
            }
            let cmd = self.input[start + 4];
            let at = start + 5;
            let gated = |s: &mut Self| -> bool {
                if !s.on_home || s.off_home_until.is_some_and(|t| Instant::now() < t) {
                    s.say("ERR:not_on_home\n");
                    return false;
                }
                true
            };
            match cmd {
                b'S' => {
                    self.input.drain(..at);
                    // The real firmware gates status on the home screen too.
                    if !gated(self) {
                        continue;
                    }
                    self.log_noise();
                    self.say(
                        "STATUS:protocol=1,device=x4,firmware=1.6.1,free=90000,largest=40000\n",
                    );
                }
                b'A' => {
                    let Some((p, end)) = self.take_path(at) else {
                        return;
                    };
                    self.input.drain(..end);
                    if !gated(self) {
                        continue;
                    }
                    if !self.dirs.contains(&p) {
                        self.say("ERR:opendir\n");
                        continue;
                    }
                    self.say(&format!("DIR:{p}\n"));
                    for (name, size) in self.children(&p) {
                        match size {
                            None => self.say(&format!("d|{name}\n")),
                            Some(n) => self.say(&format!("f|{name}|{n}|0\n")),
                        }
                        self.log_noise();
                    }
                    self.say("END\n");
                }
                b'K' => {
                    let Some((p, end)) = self.take_path(at) else {
                        return;
                    };
                    self.input.drain(..end);
                    if gated(self) {
                        self.dirs.insert(p);
                        self.say("OK\n");
                    }
                }
                b'W' => {
                    let Some((p, end)) = self.take_path(at) else {
                        return;
                    };
                    if self.input.len() < end + 4 {
                        return;
                    }
                    let size =
                        u32::from_le_bytes(self.input[end..end + 4].try_into().unwrap()) as usize;
                    self.input.drain(..end + 4);
                    if !gated(self) {
                        continue;
                    }
                    self.say("READY\n");
                    self.writing = Some(Upload {
                        path: p,
                        size,
                        data: Vec::new(),
                        waiting_crc: false,
                    });
                }
                b'T' => {
                    let Some((p, end)) = self.take_path(at) else {
                        return;
                    };
                    self.input.drain(..end);
                    if !gated(self) {
                        continue;
                    }
                    let Some(data) = self.files.get(&p).cloned() else {
                        self.say("ERR:fopen\n");
                        continue;
                    };
                    self.say("READY\n");
                    self.output.extend((data.len() as u32).to_le_bytes());
                    self.output.extend(&data);
                    let mut crc = crc32fast::hash(&data);
                    if self.corrupt_next_crc {
                        crc ^= 1;
                        self.corrupt_next_crc = false;
                    }
                    self.output.extend(crc.to_le_bytes());
                }
                b'R' => {
                    let Some((p, end)) = self.take_path(at) else {
                        return;
                    };
                    self.input.drain(..end);
                    if !gated(self) {
                        continue;
                    }
                    let prefix = format!("{p}/");
                    let existed = self.files.remove(&p).is_some() | self.dirs.remove(&p);
                    self.files.retain(|k, _| !k.starts_with(&prefix));
                    self.dirs.retain(|k| !k.starts_with(&prefix));
                    self.say(if existed { "OK\n" } else { "ERR:not_found\n" });
                }
                b'N' => {
                    let Some((src, end)) = self.take_path(at) else {
                        return;
                    };
                    let Some((dst, end)) = self.take_path(end) else {
                        return;
                    };
                    self.input.drain(..end);
                    if !gated(self) {
                        continue;
                    }
                    if let Some(d) = self.files.remove(&src) {
                        self.files.insert(dst, d);
                        self.say("OK\n");
                    } else {
                        self.say("ERR:not_found\n");
                    }
                }
                _ => {
                    self.input.drain(..at);
                    self.say("ERR:unknown_cmd\n");
                }
            }
        }
    }
}

impl Read for FakeFirmware {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.process();
        // Deliver in small, uneven pieces like a USB CDC endpoint.
        let n = buf.len().min(self.output.len()).min(61);
        for b in buf.iter_mut().take(n) {
            *b = self.output.pop_front().unwrap();
        }
        Ok(n)
    }
}

impl Write for FakeFirmware {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.input.extend_from_slice(buf);
        self.process();
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn transport() -> UsbTransport<FakeFirmware> {
    UsbTransport::new(FakeFirmware::new(), "fake".into())
}

#[test]
fn status_skips_log_noise() {
    let mut t = transport();
    let s = t.status().unwrap();
    assert_eq!(s.device, "x4");
    assert_eq!(s.firmware, "1.6.1");
}

#[test]
fn write_list_read_roundtrip() {
    let mut t = transport();
    t.mkdir("/Books").unwrap();
    // Sizes around chunk boundaries, including empty.
    for (i, len) in [0usize, 1, 255, 256, 257, 512, 100_003]
        .into_iter()
        .enumerate()
    {
        let data: Vec<u8> = (0..len).map(|x| (x * 7 + i) as u8).collect();
        let path = format!("/Books/b{i}.epub");
        let mut last = (0, 0);
        t.write(&path, &data, false, &mut |d, n| last = (d, n))
            .unwrap();
        assert_eq!(last, (len as u64, len as u64));
        assert_eq!(t.read(&path, &mut |_, _| {}).unwrap(), data);
    }
    let list = t.list("/Books").unwrap();
    assert_eq!(list.len(), 7);
    assert!(list
        .iter()
        .any(|e| e.name == "b6.epub" && e.size == 100_003 && !e.is_dir));
    let root = t.list("/").unwrap();
    assert!(root.iter().any(|e| e.name == "Books" && e.is_dir));
}

#[test]
fn overwrite_guard_rename_remove() {
    let mut t = transport();
    t.write("/a.txt", b"one", false, &mut |_, _| {}).unwrap();
    assert!(
        matches!(t.write("/a.txt", b"two", false, &mut |_, _| {}), Err(Error::Device(c)) if c == "target_exists")
    );
    t.write("/a.txt", b"two", true, &mut |_, _| {}).unwrap();
    t.rename("/a.txt", "/b.txt").unwrap();
    assert_eq!(t.read("/b.txt", &mut |_, _| {}).unwrap(), b"two");
    t.remove("/b.txt").unwrap();
    assert!(t.list("/").unwrap().is_empty());
}

#[test]
fn not_on_home_is_reported() {
    let mut fw = FakeFirmware::new();
    fw.on_home = false;
    let mut t = UsbTransport::new(fw, "fake".into());
    let err = t.list("/").unwrap_err();
    assert!(err.is_not_on_home(), "{err}");
    assert!(err.to_string().contains("home screen"));
}

#[test]
fn corrupted_read_is_detected() {
    let mut t = transport();
    t.write("/x.bin", &[1, 2, 3], true, &mut |_, _| {}).unwrap();
    // Reach into the fake through a fresh transport to flip the CRC.
    let mut fw = FakeFirmware::new();
    fw.files.insert("/x.bin".into(), vec![1, 2, 3]);
    fw.corrupt_next_crc = true;
    let mut t = UsbTransport::new(fw, "fake".into());
    assert!(matches!(t.read("/x.bin", &mut |_, _| {}), Err(Error::Device(c)) if c == "crc"));
}

#[test]
fn batch_survives_a_busy_reader_between_files() {
    let mut fw = FakeFirmware::new();
    fw.settle_after_write = true;
    let mut t = UsbTransport::new(fw, "fake".into());
    t.mkdir("/Books").unwrap();
    let started = Instant::now();
    for i in 0..3 {
        let data: Vec<u8> = (0..5000).map(|x| (x + i) as u8).collect();
        write_in_batch(
            &mut t,
            &format!("/Books/b{i}.epub"),
            &data,
            true,
            &mut |_, _| {},
        )
        .unwrap();
    }
    // Each file waited out the busy + off-home window of the previous one.
    assert!(
        started.elapsed() >= Duration::from_millis(3500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(t.list("/Books").unwrap().len(), 3);
}
