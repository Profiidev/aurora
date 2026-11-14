import CoreBluetooth
import Foundation

/// Bluetooth LE fallback: scans for desktops advertising the Aurora service,
/// opens an L2CAP channel to the ones Rust wants and moves their bytes
/// through the `aurora_bt_*` functions (crates/mobile/src/bluetooth_ios.rs).
/// Must match crates/transport/src/bluetooth/mod.rs.
final class AuroraBluetooth: NSObject {
    static let shared = AuroraBluetooth()

    private static let service = CBUUID(string: "6a1f3c2e-8b4d-4e7a-9c50-aa0b7d31e5c4")
    /// Manufacturer data starts with this company id, little-endian
    private static let company: [UInt8] = [0xff, 0xff]

    private var central: CBCentralManager?
    private var timer: Timer?
    private var active = false
    /// Peripherals being connected or linked, with the PSM to open
    private var peripherals: [UUID: (CBPeripheral, UInt16)] = [:]
    private var channels: [UInt64: Link] = [:]
    private var nextHandle: UInt64 = 1

    /// One L2CAP channel and its streams
    private final class Link: NSObject, StreamDelegate {
        let handle: UInt64
        let channel: CBL2CAPChannel
        let peripheral: CBPeripheral
        var outgoing = [UInt8](repeating: 0, count: 16 * 1024)
        var unsent = Data()
        weak var owner: AuroraBluetooth?

        init(handle: UInt64, channel: CBL2CAPChannel, peripheral: CBPeripheral) {
            self.handle = handle
            self.channel = channel
            self.peripheral = peripheral
        }

        func open() {
            for stream in [channel.inputStream as Stream, channel.outputStream as Stream] {
                stream.delegate = self
                stream.schedule(in: .main, forMode: .default)
                stream.open()
            }
        }

        func close() {
            channel.inputStream.close()
            channel.outputStream.close()
        }

        func stream(_ stream: Stream, handle event: Stream.Event) {
            switch event {
            case .hasBytesAvailable:
                var buf = [UInt8](repeating: 0, count: 16 * 1024)
                while channel.inputStream.hasBytesAvailable {
                    let n = channel.inputStream.read(&buf, maxLength: buf.count)
                    if n <= 0 { break }
                    buf.withUnsafeBufferPointer { aurora_bt_received(handle, $0.baseAddress, n) }
                }
            case .hasSpaceAvailable:
                pump()
            case .errorOccurred, .endEncountered:
                owner?.drop(handle)
            default:
                break
            }
        }

        /// Moves the link's bytes into the output stream; false once over
        @discardableResult
        func pump() -> Bool {
            while true {
                if unsent.isEmpty {
                    let n = outgoing.withUnsafeMutableBufferPointer {
                        aurora_bt_take_outgoing(handle, $0.baseAddress, $0.count)
                    }
                    if n < 0 { return false }
                    if n == 0 { return true }
                    unsent = Data(outgoing[0..<n])
                }
                guard channel.outputStream.hasSpaceAvailable else { return true }
                let written = unsent.withUnsafeBytes {
                    channel.outputStream.write($0.bindMemory(to: UInt8.self).baseAddress!, maxLength: unsent.count)
                }
                if written <= 0 { return true }
                unsent.removeFirst(written)
            }
        }
    }

    /// From the scene: Bluetooth only runs while the app is active
    func setActive(_ on: Bool) {
        active = on
        aurora_bt_set_foreground(on)
        if on {
            if central == nil {
                central = CBCentralManager(delegate: self, queue: .main)
            }
            timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
                self?.tick()
            }
        } else {
            timer?.invalidate()
            timer = nil
            central?.stopScan()
            for handle in Array(channels.keys) { drop(handle) }
            for (peripheral, _) in peripherals.values { central?.cancelPeripheralConnection(peripheral) }
            peripherals.removeAll()
        }
    }

    private var ticks = 0

    private func tick() {
        // links that ended on the Rust side
        for (handle, link) in channels where !link.pump() { drop(handle) }
        ticks += 1
        // scanning decisions once a second
        guard ticks % 20 == 0, let central, central.state == .poweredOn else { return }
        if active && aurora_bt_wanted() {
            if !central.isScanning {
                central.scanForPeripherals(withServices: [Self.service])
            }
        } else if central.isScanning {
            central.stopScan()
        }
    }

    fileprivate func drop(_ handle: UInt64) {
        guard let link = channels.removeValue(forKey: handle) else { return }
        link.close()
        aurora_bt_closed(handle)
        peripherals.removeValue(forKey: link.peripheral.identifier)
        central?.cancelPeripheralConnection(link.peripheral)
    }
}

extension AuroraBluetooth: CBCentralManagerDelegate, CBPeripheralDelegate {
    func centralManagerDidUpdateState(_ central: CBCentralManager) {}

    func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral,
                        advertisementData: [String: Any], rssi RSSI: NSNumber) {
        guard peripherals[peripheral.identifier] == nil,
              let data = advertisementData[CBAdvertisementDataManufacturerDataKey] as? Data,
              data.count > 2, Array(data.prefix(2)) == Self.company else { return }
        let payload = [UInt8](data.dropFirst(2))
        var psm: UInt16 = 0
        let dial = payload.withUnsafeBufferPointer {
            aurora_bt_should_dial($0.baseAddress, $0.count, &psm)
        }
        guard dial else { return }
        peripherals[peripheral.identifier] = (peripheral, psm)
        peripheral.delegate = self
        central.connect(peripheral)
    }

    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        guard let entry = peripherals[peripheral.identifier] else { return }
        peripheral.openL2CAPChannel(CBL2CAPPSM(entry.1))
    }

    func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        peripherals.removeValue(forKey: peripheral.identifier)
    }

    func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral,
                        error: Error?) {
        for (handle, link) in channels where link.peripheral.identifier == peripheral.identifier {
            drop(handle)
        }
        peripherals.removeValue(forKey: peripheral.identifier)
    }

    func peripheral(_ peripheral: CBPeripheral, didOpen channel: CBL2CAPChannel?, error: Error?) {
        guard let channel, error == nil else {
            peripherals.removeValue(forKey: peripheral.identifier)
            central?.cancelPeripheralConnection(peripheral)
            return
        }
        let handle = nextHandle
        nextHandle += 1
        let link = Link(handle: handle, channel: channel, peripheral: peripheral)
        link.owner = self
        channels[handle] = link
        aurora_bt_opened(handle)
        link.open()
    }
}
