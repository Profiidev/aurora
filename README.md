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
| `aurora_transport`     | lib                          | `Transport` trait; features `iroh` (network), `bluer` (bluetooth, Linux), `nusb` (usb) |
| `aurora_ui`            | lib                          | shared gpui theme and views                                                            |

Native shells and packaging live in `platform/{android,ios,packaging}`.
Dependencies and package metadata are declared once in the root `Cargo.toml`.

## Feature -> crate

- Pairing, mTLS, BLAKE3, permissions, device store: `aurora_core` + `aurora_proto`
- LAN/relay/USB/BLE links, reconnect: `aurora_transport`
- Notifications, SMS, calls, media, files, clipboard, commands, battery, input: `aurora_core` feature modules; desktop side in `aurora_daemon`, phone side in `aurora_mobile`
- Screen mirror, remote control, webcam, second display: `aurora_viewer`
- Tray, panel, settings: `aurora_gui` + `aurora_ui`

## Commands

`just desktop`, `just daemon`, `just android`, `just android-release`, `just ios`, `just icon <png>`.

## License

GPL-2.0, see [LICENSE](LICENSE).
