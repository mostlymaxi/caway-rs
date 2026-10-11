//! caway: an MPRIS now-playing module for waybar with a cava equalizer.
//!
//! One process, one thread, blocking in poll(2) between events. It talks to
//! the session bus directly instead of running playerctl and jq, and only runs
//! cava while something is actually playing.

mod dbus;
mod sys;

use dbus::{Conn, Message, Value};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
/// How long the track title is shown before the equalizer takes over.
const TITLE_HOLD: Duration = Duration::from_secs(2);
const BARS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
const THIN_SPACE: &str = "\u{2009}";

struct Opts {
    bars: u32,
    framerate: u32,
    equalizer: bool,
}

fn parse_args() -> Opts {
    let mut o = Opts { bars: 16, framerate: 30, equalizer: true };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let num = |v: Option<String>| v.and_then(|v| v.parse().ok()).filter(|&n| n > 0);
        match a.as_str() {
            "-b" => o.bars = num(args.next()).unwrap_or_else(|| usage(1)),
            "-f" => o.framerate = num(args.next()).unwrap_or_else(|| usage(1)),
            "-e" => o.equalizer = false,
            "-h" | "--help" => usage(0),
            _ => usage(1),
        }
    }
    o
}

fn usage(code: i32) -> ! {
    let msg = "caway usage: caway [ options ... ]\n\
        where options include:\n\n  \
        -b <integer>  (Number of bars to display. Default 16)\n  \
        -f <integer>  (Framerate of the equalizer. Default 30)\n  \
        -e            (Disable equalizer. Default enabled)\n  \
        -h            (Show help message)\n";
    if code == 0 { print!("{msg}") } else { eprint!("{msg}") }
    std::process::exit(code)
}

#[derive(Default)]
struct Player {
    owner: String,
    status: String,
    artist: String,
    title: String,
    /// Logical clock of the last change, used to pick the active player.
    touched: u64,
}

enum Pending {
    ListNames,
    GetAll(String),
}

struct App {
    opts: Opts,
    bus: Conn,
    players: HashMap<String, Player>,
    pending: HashMap<u32, Pending>,
    clock: u64,
    /// What the module currently shows when the equalizer is not active.
    shown: String,
    tooltip: String,
    cava: Option<Child>,
    cava_buf: Vec<u8>,
    cava_at: Option<Instant>,
    last_bars: String,
    out: io::Stdout,
    line: String,
}

fn main() {
    let opts = parse_args();
    if let Err(e) = run(opts) {
        eprintln!("caway: {e}");
        std::process::exit(1);
    }
}

fn run(opts: Opts) -> io::Result<()> {
    let mut bus = Conn::session(sys::getuid())?;
    for rule in [
        format!("type='signal',interface='{PROPS_IFACE}',member='PropertiesChanged',path='{MPRIS_PATH}',arg0='{PLAYER_IFACE}'"),
        "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0namespace='org.mpris.MediaPlayer2'".to_owned(),
    ] {
        bus.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "AddMatch", &[&rule])?;
    }
    let serial = bus.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames", &[])?;

    let mut app = App {
        opts,
        bus,
        players: HashMap::new(),
        pending: HashMap::from([(serial, Pending::ListNames)]),
        clock: 0,
        shown: String::new(),
        tooltip: String::new(),
        cava: None,
        cava_buf: Vec::new(),
        cava_at: None,
        last_bars: String::new(),
        out: io::stdout(),
        line: String::with_capacity(512),
    };
    app.event_loop()
}

impl App {
    fn event_loop(&mut self) -> io::Result<()> {
        let mut msgs = Vec::new();
        loop {
            let timeout = match self.cava_at {
                Some(t) => t.saturating_duration_since(Instant::now()).as_millis().min(i32::MAX as u128) as i32,
                None => -1,
            };
            let cava_fd = self.cava.as_ref().and_then(|c| c.stdout.as_ref()).map(|s| {
                use std::os::fd::AsRawFd;
                s.as_raw_fd()
            });
            let mut fds = [
                sys::PollFd { fd: self.bus.fd(), events: sys::POLLIN, revents: 0 },
                sys::PollFd { fd: cava_fd.unwrap_or(-1), events: sys::POLLIN, revents: 0 },
            ];
            sys::poll(&mut fds, timeout)?;

            if fds[0].revents != 0 {
                if !self.bus.read_messages(|m| msgs.push(m))? {
                    return Err(io::Error::other("session bus closed"));
                }
                for m in msgs.drain(..) {
                    self.handle(m)?;
                }
                self.refresh()?;
            }
            if fds[1].revents != 0 {
                self.read_cava()?;
            }
            if self.cava_at.is_some_and(|t| Instant::now() >= t) {
                self.cava_at = None;
                self.start_cava();
            }
        }
    }

    fn handle(&mut self, m: Message) -> io::Result<()> {
        match m.kind {
            dbus::METHOD_RETURN | dbus::ERROR => match self.pending.remove(&m.reply_serial) {
                Some(Pending::ListNames) if m.kind == dbus::METHOD_RETURN => {
                    let names: Vec<String> = m.body.first().and_then(Value::as_array).unwrap_or(&[]).iter()
                        .filter_map(|v| v.as_str()).filter(|n| is_player(n)).map(str::to_owned).collect();
                    for n in names {
                        // The owner is learned from GetAll's reply sender.
                        self.players.entry(n.clone()).or_default();
                        self.get_all(&n)?;
                    }
                }
                Some(Pending::GetAll(name)) => {
                    if m.kind == dbus::ERROR {
                        self.players.remove(&name);
                    } else if let Some(p) = self.players.get_mut(&name) {
                        p.owner = m.sender;
                        if let Some(props) = m.body.first() {
                            self.clock += 1;
                            apply(p, props, self.clock);
                        }
                    }
                }
                _ => {}
            },
            dbus::SIGNAL if m.member == "NameOwnerChanged" => {
                let s = |i: usize| m.body.get(i).and_then(Value::as_str).unwrap_or("");
                let (name, new) = (s(0), s(2));
                if !is_player(name) {
                } else if new.is_empty() {
                    self.players.remove(name);
                } else {
                    let name = name.to_owned();
                    self.players.entry(name.clone()).or_default().owner = new.to_owned();
                    self.get_all(&name)?;
                }
            }
            dbus::SIGNAL if m.member == "PropertiesChanged" && m.path == MPRIS_PATH => {
                if m.body.first().and_then(Value::as_str) != Some(PLAYER_IFACE) {
                    return Ok(());
                }
                let invalidated = m.body.get(2).and_then(Value::as_array).is_some_and(|a| !a.is_empty());
                let Some(name) = self.players.iter().find(|(_, p)| p.owner == m.sender).map(|(n, _)| n.clone()) else {
                    return Ok(());
                };
                if let (Some(p), Some(props)) = (self.players.get_mut(&name), m.body.get(1)) {
                    self.clock += 1;
                    apply(p, props, self.clock);
                }
                if invalidated {
                    self.get_all(&name)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn get_all(&mut self, name: &str) -> io::Result<()> {
        let s = self.bus.call(name, MPRIS_PATH, PROPS_IFACE, "GetAll", &[PLAYER_IFACE])?;
        self.pending.insert(s, Pending::GetAll(name.to_owned()));
        Ok(())
    }

    /// Recomputes what the module should show and emits it if it changed.
    fn refresh(&mut self) -> io::Result<()> {
        // Wait for the initial player scan so startup does not flash "Stopped".
        if !self.pending.is_empty() {
            return Ok(());
        }
        // A playing player wins; otherwise the one that changed last.
        let active = self.players.iter()
            .filter(|(_, p)| !p.status.is_empty())
            .max_by_key(|(_, p)| (p.status == "Playing", p.touched));

        self.line.clear();
        let playing = match active {
            Some((name, p)) => {
                let text = if p.artist.is_empty() { p.title.clone() } else { format!("{} - {}", p.artist, p.title) };
                let player = name.strip_prefix(MPRIS_PREFIX).unwrap_or(name);
                self.tooltip = format!("{player} : {text}");
                write_json(&mut self.line, &text, &self.tooltip, &p.status);
                p.status == "Playing"
            }
            None => {
                self.tooltip.clear();
                write_json(&mut self.line, "", "", "Stopped");
                false
            }
        };
        if self.line == self.shown {
            return Ok(());
        }
        std::mem::swap(&mut self.line, &mut self.shown);
        self.out.write_all(self.shown.as_bytes())?;
        self.out.flush()?;

        // Any change of track or state shows the title again first.
        self.stop_cava();
        if playing && self.opts.equalizer {
            self.cava_at = Some(Instant::now() + TITLE_HOLD);
        }
        Ok(())
    }

    fn start_cava(&mut self) {
        let conf = format!(
            "[general]\nframerate = {}\nbars = {}\nsleep_timer = 1\n\
             [output]\nmethod = raw\nraw_target = /dev/stdout\ndata_format = ascii\n\
             ascii_max_range = 7\nbar_delimiter = 59\nframe_delimiter = 10\n",
            self.opts.framerate, self.opts.bars
        );
        let path = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()) + "/caway-cava.conf";
        if let Err(e) = std::fs::write(&path, conf) {
            eprintln!("caway: cannot write {path}: {e}");
            return;
        }
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new("cava");
        cmd.arg("-p").arg(&path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        // SAFETY: die_with_parent only issues prctl, which is async-signal-safe.
        unsafe { cmd.pre_exec(|| { sys::die_with_parent(); Ok(()) }) };
        match cmd.spawn() {
            Ok(c) => self.cava = Some(c),
            Err(e) => eprintln!("caway: cannot start cava: {e}"),
        }
    }

    fn stop_cava(&mut self) {
        self.cava_at = None;
        if let Some(mut c) = self.cava.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.cava_buf.clear();
        self.last_bars.clear();
    }

    fn read_cava(&mut self) -> io::Result<()> {
        let Some(out) = self.cava.as_mut().and_then(|c| c.stdout.as_mut()) else { return Ok(()) };
        let mut tmp = [0u8; 4096];
        let n = match out.read(&mut tmp) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Ok(()),
            Err(_) => 0,
        };
        if n == 0 {
            // cava exited (no audio source, crash). Fall back to the title.
            self.stop_cava();
            return Ok(());
        }
        self.cava_buf.extend_from_slice(&tmp[..n]);

        // Only the newest complete frame matters; older ones would be stale.
        let Some(end) = self.cava_buf.iter().rposition(|&b| b == b'\n') else { return Ok(()) };
        let start = self.cava_buf[..end].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let mut bars = String::with_capacity(self.opts.bars as usize * 6);
        for v in self.cava_buf[start..end].split(|&b| b == b';').filter(|s| !s.is_empty()) {
            let lvl = v.iter().fold(0usize, |a, &d| a * 10 + d.wrapping_sub(b'0') as usize).min(7);
            if !bars.is_empty() {
                bars.push_str(THIN_SPACE);
            }
            bars.push_str(BARS[lvl]);
        }
        self.cava_buf.drain(..=end);

        // Silence produces identical frames; skip them so waybar stays idle.
        if bars == self.last_bars {
            return Ok(());
        }
        self.line.clear();
        write_json(&mut self.line, &bars, &self.tooltip, "Playing");
        self.out.write_all(self.line.as_bytes())?;
        self.out.flush()?;
        self.last_bars = bars;
        Ok(())
    }
}

fn is_player(name: &str) -> bool {
    // playerctld only mirrors other players; following it would double count.
    name.starts_with(MPRIS_PREFIX) && name != "org.mpris.MediaPlayer2.playerctld"
}

fn apply(p: &mut Player, props: &Value, clock: u64) {
    for (k, v) in props.dict() {
        match k {
            "PlaybackStatus" => p.status = v.as_str().unwrap_or("").to_owned(),
            "Metadata" => {
                p.artist.clear();
                p.title.clear();
                for (mk, mv) in v.dict() {
                    match mk {
                        "xesam:title" => p.title = mv.as_str().unwrap_or("").to_owned(),
                        "xesam:artist" => {
                            let artists = mv.as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>());
                            p.artist = match (artists, mv.as_str()) {
                                (Some(a), _) => a.join(", "),
                                (None, Some(s)) => s.to_owned(),
                                _ => String::new(),
                            };
                        }
                        _ => {}
                    }
                }
            }
            _ => continue,
        }
        p.touched = clock;
    }
}

fn write_json(out: &mut String, text: &str, tooltip: &str, status: &str) {
    out.push_str("{\"text\":\"");
    escape_json(out, text);
    out.push_str("\",\"tooltip\":\"");
    escape_json(out, tooltip);
    out.push_str("\",\"alt\":\"");
    escape_json(out, status);
    out.push_str("\",\"class\":\"");
    escape_json(out, status);
    out.push_str("\"}\n");
}

fn escape_json(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}
