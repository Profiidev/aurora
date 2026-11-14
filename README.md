# Aurora

Phone <-> desktop suite: notifications, messaging, files, clipboard, media, screen mirror, input and more.

## Layout

| Crate                  | Kind                         | Role                                                                                  |
| ---------------------- | ---------------------------- | ------------------------------------------------------------------------------------- |
| `aurora` (root `src/`) | bin `aurora`                 | CLI entry, calls `aurora_cli`                                                         |
| `aurora_cli`           | lib                          | clap commands, talk to aurorad over a local socket                                    |
| `aurora_daemon`        | bin `aurorad`                | background daemon (systemd user service), Linux only                                  |
| `aurora_gui`           | bin `aurora-gui`             | desktop app ("Aurora"), embeds the viewer                                             |
| `aurora_mobile`        | cdylib/staticlib `libaurora` | Android + iOS app ("Aurora")                                                          |
| `aurora_viewer`        | lib + bin `aurora-viewer`    | phone screen mirror view                                                              |
| `aurora_core`          | lib                          | pairing, crypto, device store, config, IPC types, feature modules (cargo features)    |
| `aurora_proto`         | lib                          | postcard wire messages                                                                |
| `aurora_transport`     | lib                          | iroh node; links (USB, Bluetooth) as iroh custom transports; features `nusb`, `bluer` |
| `aurora_ui`            | lib                          | shared gpui theme and views                                                           |

Native shells and packaging live in `platform/{android,ios,packaging}`.
Dependencies and package metadata are declared once in the root `Cargo.toml`.

## Feature -> crate

- Pairing, mTLS, BLAKE3, permissions, device store: `aurora_core` + `aurora_proto`
- LAN/relay/USB/BLE links, reconnect: `aurora_transport`
- Notifications, SMS, calls, media, files, clipboard, commands, battery, input: `aurora_core` feature modules; desktop side in `aurora_daemon`, phone side in `aurora_mobile`
- Screen mirror, remote control, webcam, second display: `aurora_viewer`
- Tray, panel, settings: `aurora_gui` + `aurora_ui`

## USB

Android phones connect over a cable through Android Open Accessory mode,
no USB debugging needed. `aurorad` switches phones from known vendors to
accessory mode; Android then asks to open Aurora, which runs the link. A
cabled phone shows up for pairing even with discovery off, and a paired one
keeps its network connection, so pulling the cable drops nothing. iOS has no
USB connection. Switch it off with `[transport] usb = false` or in settings.

On Linux, the user needs access to the phone's USB device. The .deb and the
NixOS module install `platform/packaging/70-aurora.rules`; for other
installs copy it to `/etc/udev/rules.d/` and run
`sudo udevadm control --reload && sudo udevadm trigger`.

## Bluetooth

A Bluetooth LE fallback for when neither Wi-Fi nor USB reaches a paired
device. It only runs while an Aurora window is open (the desktop GUI, or the
phone app in the foreground), the setting is on, and a paired device is
offline, or the pair page waits with discovery on. aurorad advertises the
Aurora service through BlueZ and accepts L2CAP channels; the phone scans
and dials. As soon as the device is reachable over USB or the network, the
Bluetooth link is dropped.

The desktop needs BlueZ running and the adapter switched on (Aurora never
switches it on). Android 12 or newer (the app's minimum) and iOS ask for the
Bluetooth permission the first time it is needed. Switch it off with
`[transport] bluetooth = false` or in settings.

## Android background

On Android the engine runs in a foreground service in its own process
(`:engine`), the phone's counterpart of aurorad: the app UI talks to it over
the same local IPC socket, so the phone stays connected while the app is
closed and the engine starts again after a reboot. Its notification goes to
a silent channel and Aurora never asks for the notification permission, so
it does not show in the shade. "Run in background" (`[mobile] background`,
default on) turns this off: the engine then only runs while the app is open.
The app asks once to be exempted from battery optimization.

iOS has no equivalent: it suspends apps shortly after they leave the screen,
and its background exceptions (Bluetooth, periodic refresh, push wake-ups)
cannot hold a live connection. On iOS Aurora is connected only while the app
is open.

## Peer messages

Paired devices talk through types registered in
`crates/core/src/peer/messages.rs`, like the IPC commands: implement
`PeerMessage` (one-way), `PeerRequest` (answered) or `PeerStream` (typed
channel), add it to `register_all`, and send it with `Engine::notify`,
`Engine::request` or `Engine::open_stream`.

## Commands

`just desktop`, `just daemon`, `just android`, `just android-release`, `just ios`, `just icon <png>`.

## License

GPL-2.0, see [LICENSE](LICENSE).
