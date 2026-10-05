//! The Network page the installer and the first-boot setup share: whether
//! the machine is online, what each wired port is doing, and the Wi-Fi
//! networks in range, with a passphrase field for the one picked.
//!
//! The talking to networkd's files and wpa_supplicant happens on a thread of
//! its own (see [`crate::network`]), so a slow or restarting daemon never
//! holds up a frame; the page reads the latest [`Status`] it published.

use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use egui::{RichText, Ui};
use mcsapi::toolkit::egui;
use mcsapi_components::{Button, ButtonVariant, Tokens};

use crate::{
    network::{self, Control, Network, Wired, WiredState},
    wizard::{self, Row},
};

/// How long a scan is given before its results are final.
const SCAN_TIME: Duration = Duration::from_secs(4);
/// Association, the handshake and DHCP together. A wrong passphrase looks
/// like a handshake that never completes, so this is also how long it takes
/// to say so.
const CONNECT_TIME: Duration = Duration::from_secs(30);
/// How often the worker looks again.
const POLL: Duration = Duration::from_millis(500);

/// What the worker last saw.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// Whether some link is routable.
    pub online: bool,
    /// The wired ports.
    pub wired: Vec<Wired>,
    /// The Wi-Fi interface, if there is one.
    pub interface: Option<String>,
    /// Whether wpa_supplicant answers on it.
    pub wifi_ready: bool,
    /// Networks in range, strongest first.
    pub networks: Vec<Network>,
    /// Whether a scan is still running.
    pub scanning: bool,
    /// The network being joined.
    pub joining: Option<String>,
    /// The network wpa_supplicant is associated with.
    pub connected: Option<String>,
    /// The last thing that went wrong, as a sentence.
    pub message: Option<String>,
}

enum Order {
    Scan,
    Join(Network, String),
}

/// The network, watched from a thread of its own.
pub struct Wifi {
    status: Arc<Mutex<Status>>,
    orders: Sender<Order>,
}

impl std::fmt::Debug for Wifi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wifi").finish_non_exhaustive()
    }
}

impl Wifi {
    /// Starts watching. With `save`, a network joined is written to
    /// wpa_supplicant's configuration so the installed system joins it
    /// again after a reboot; a live installer has nothing to save it to.
    pub fn start(save: bool) -> Self {
        let status = Arc::new(Mutex::new(Status::default()));
        let (orders, rx) = mpsc::channel();
        let shared = Arc::clone(&status);
        thread::Builder::new()
            .name("derisk-wifi".into())
            .spawn(move || worker(&shared, &rx, save))
            .expect("a thread can be started");
        Self { status, orders }
    }

    /// What the worker last saw.
    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Looks for networks again.
    pub fn scan(&self) {
        let _ = self.orders.send(Order::Scan);
    }

    /// Joins `network` with `passphrase` (ignored for open networks).
    pub fn join(&self, network: Network, passphrase: String) {
        let _ = self.orders.send(Order::Join(network, passphrase));
    }
}

struct Joining {
    name: String,
    id: u32,
    since: Instant,
}

fn worker(shared: &Mutex<Status>, orders: &Receiver<Order>, save: bool) {
    let mut control: Option<Control> = None;
    let mut scan_since: Option<Instant> = None;
    let mut joining: Option<Joining> = None;
    loop {
        let order = match orders.recv_timeout(POLL) {
            Ok(order) => Some(order),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let mut status = shared.lock().map(|s| s.clone()).unwrap_or_default();
        status.online = network::online(Path::new(network::NETIF_STATE));
        status.wired = network::wired(
            Path::new(network::SYS_CLASS_NET),
            Path::new(network::NETIF_LINKS),
        );

        if control.is_none() {
            status.interface = network::interfaces(Path::new(network::SYS_CLASS_NET))
                .into_iter()
                .next();
            // wpa_supplicant makes its socket a moment after the interface
            // appears, so this is tried again every poll until it is there.
            control = status
                .interface
                .as_deref()
                .and_then(|i| Control::open(i).ok());
            if let Some(c) = &control
                && c.request("SCAN").is_ok()
            {
                scan_since = Some(Instant::now());
            }
        }
        // NixOS restarts wpa_supplicant whenever a wireless interface comes
        // or goes, and the restarted daemon listens on a new socket.
        if control.as_ref().is_some_and(|c| c.request("PING").is_err()) {
            control = None;
            joining = None;
        }
        status.wifi_ready = control.is_some();

        if let Some(c) = &control {
            match order {
                // A reply of FAIL-BUSY means a scan is already running,
                // which serves as well.
                Some(Order::Scan) => {
                    let _ = c.request("SCAN");
                    scan_since = Some(Instant::now());
                }
                Some(Order::Join(network, passphrase)) => {
                    if let Some(old) = joining.take() {
                        let _ = c.forget(old.id);
                    }
                    match c.connect(&network, &passphrase) {
                        Ok(id) => {
                            status.message = None;
                            joining = Some(Joining {
                                name: network.name(),
                                id,
                                since: Instant::now(),
                            });
                        }
                        Err(e) => status.message = Some(e.to_string()),
                    }
                }
                None => {}
            }
            if scan_since.is_some()
                && let Ok(reply) = c.request("SCAN_RESULTS")
            {
                status.networks = network::parse_scan_results(&reply);
            }
            if scan_since.is_some_and(|t| t.elapsed() > SCAN_TIME) {
                scan_since = None;
            }
            let wpa = c.status().unwrap_or_default();
            let get = |key: &str| wpa.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
            status.connected = (get("wpa_state").as_deref() == Some("COMPLETED"))
                .then(|| {
                    get("ssid")
                        .map(|s| String::from_utf8_lossy(&network::decode_ssid(&s)).into_owned())
                })
                .flatten();
            if let Some(j) = &joining {
                if status.connected.as_deref() == Some(j.name.as_str()) {
                    if save && let Err(e) = c.save() {
                        status.message = Some(format!("Joined, but not saved: {e}"));
                    }
                    joining = None;
                } else if j.since.elapsed() > CONNECT_TIME {
                    let _ = c.forget(j.id);
                    status.message = Some(format!(
                        "Could not join {}. Check the passphrase and try again.",
                        j.name
                    ));
                    joining = None;
                }
            }
        }
        status.scanning = scan_since.is_some();
        status.joining = joining.as_ref().map(|j| j.name.clone());
        if let Ok(mut s) = shared.lock() {
            *s = status;
        }
    }
}

/// The Network page's own state: the network picked and its passphrase.
#[derive(Debug, Default)]
pub struct NetworkPage {
    picked: Option<Vec<u8>>,
    passphrase: String,
}

impl NetworkPage {
    /// Draws the page body from `status`, sending scans and joins to `wifi`.
    pub fn show(&mut self, ui: &mut Ui, wifi: &Wifi, status: &Status) {
        let tokens = Tokens::current(ui.ctx());
        let headline = if status.online {
            match &status.connected {
                Some(name) => format!("Online, through {name}."),
                None => "Online.".to_owned(),
            }
        } else if status.joining.is_some() {
            format!("Joining {}…", status.joining.as_deref().unwrap_or_default())
        } else {
            "Not connected.".to_owned()
        };
        ui.label(RichText::new(headline).strong());
        for port in &status.wired {
            let line = format!("Cable ({}): {}", port.name, port.state.label());
            let color = if port.state == WiredState::Online {
                tokens.foreground
            } else {
                tokens.muted_foreground
            };
            ui.label(RichText::new(line).color(color).font(tokens.small_font()));
        }
        ui.add_space(8.0);
        if let Some(message) = &status.message {
            wizard::notice(ui, message, true);
            ui.add_space(8.0);
        }

        let Some(_) = &status.interface else {
            wizard::notice(
                ui,
                if status.online {
                    "No Wi-Fi adapter was found."
                } else if status.wired.is_empty() {
                    "This machine has no network adapter that works yet."
                } else {
                    "No Wi-Fi adapter was found. Plug in a network cable."
                },
                false,
            );
            return;
        };
        if !status.wifi_ready {
            wizard::busy(ui, "Starting Wi-Fi…");
            return;
        }

        // The passphrase for the network picked, above the list so the
        // on-screen keyboard does not cover it.
        let picked = self
            .picked
            .as_ref()
            .and_then(|ssid| status.networks.iter().find(|n| &n.ssid == ssid))
            .cloned();
        if let Some(network) = &picked {
            if network.security.needs_passphrase() {
                let field = wizard::field(
                    ui,
                    &format!("Passphrase for {}", network.name()),
                    &mut self.passphrase,
                    wizard::Entry::Secret,
                    "Passphrase",
                );
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    wifi.join(network.clone(), std::mem::take(&mut self.passphrase));
                }
            }
            ui.horizontal(|ui| {
                let joining = status.joining.is_some();
                if ui.add(Button::new("Join").enabled(!joining)).clicked() {
                    wifi.join(network.clone(), std::mem::take(&mut self.passphrase));
                }
                if joining {
                    wizard::busy(ui, "Joining…");
                }
            });
            ui.add_space(8.0);
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Wi-Fi networks").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if status.scanning {
                    ui.add(mcsapi_components::Spinner::new());
                } else if ui
                    .add(Button::new("Scan again").variant(ButtonVariant::Ghost))
                    .clicked()
                {
                    wifi.scan();
                }
            });
        });
        ui.add_space(4.0);
        let selected = picked
            .as_ref()
            .and_then(|p| status.networks.iter().position(|n| n.ssid == p.ssid));
        let networks = &status.networks;
        if let Some(i) = wizard::list(ui, "wifi", networks.len(), selected, None, |i| {
            let n = &networks[i];
            let mut detail = format!("{} · {}", bars(n.signal), n.security.label());
            if status.connected.as_deref() == Some(n.name().as_str()) {
                detail = format!("Connected · {detail}");
            }
            Row {
                title: n.name(),
                detail,
            }
        }) {
            let network = networks[i].clone();
            self.passphrase.clear();
            self.picked = Some(network.ssid.clone());
            // An open network needs nothing typed: join it on the tap.
            if !network.security.needs_passphrase() {
                wifi.join(network, String::new());
            }
        }
    }
}

/// Signal strength as one to four bars.
fn bars(dbm: i32) -> &'static str {
    match dbm {
        d if d >= -55 => "▂▄▆█",
        d if d >= -67 => "▂▄▆",
        d if d >= -78 => "▂▄",
        _ => "▂",
    }
}
