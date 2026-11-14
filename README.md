# Aurora

Phone <-> desktop suite: notifications, messaging, files, clipboard, media, screen mirror, input and more.

## Layout

| Crate                  | Kind                         | Role                                                                                   |
| ---------------------- | ---------------------------- | -------------------------------------------------------------------------------------- |
| `aurora` (root `src/`) | bin `aurora`                 | CLI entry, calls `aurora_cli`                                                          |
| `aurora_cli`           | lib                          | clap commands, talk to aurorad over a local socket                                     |
| `aurora_daemon`        | bin `aurorad`                | background daemon (systemd user service), Linux only                                   |
| `aurora_gui`           | bin `aurora-gui`             | desktop app ("Aurora"), embeds the viewer                                              |
| `aurora_mobile`        | cdylib/staticlib `libaurora` | Android + iOS app ("Aurora")                                                           |
| `aurora_viewer`        | lib + bin `aurora-viewer`    | phone screen mirror view                                                               |
| `aurora_core`          | lib                          | pairing, crypto, device store, config, IPC types, feature modules (cargo features)     |
| `aurora_proto`         | lib                          | postcard wire messages                                                                 |
| `aurora_transport`     | lib                          | iroh node; links (USB) as iroh custom transports; feature `nusb` for the USB host side |
| `aurora_ui`            | lib                          | shared gpui theme and views                                                            |

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
