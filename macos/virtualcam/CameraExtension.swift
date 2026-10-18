// Cutaway Virtual Camera: a CoreMediaIO camera extension.
//
// It has two streams: a sink that Cutaway feeds with the program picture,
// and the source that video apps (Zoom, Teams, Meet, FaceTime, OBS...)
// see as a camera called "Cutaway". When Cutaway isn't sending, the camera
// shows a dark "Cutaway isn't running" card instead of freezing.

import AppKit
import CoreMediaIO
import Foundation
import IOKit.audio
import os.log

let kWidth: Int32 = 1920
let kHeight: Int32 = 1080
let kFrameRate: Int32 = 30
let kDeviceID = UUID(uuidString: "4E7A6C0B-2B0E-4C55-9C0A-5A1A1D1C0A11")!
let kSourceID = UUID(uuidString: "4E7A6C0B-2B0E-4C55-9C0A-5A1A1D1C0A12")!
let kSinkID = UUID(uuidString: "4E7A6C0B-2B0E-4C55-9C0A-5A1A1D1C0A13")!
let log = OSLog(subsystem: "app.cutaway.Cutaway.Camera", category: "camera")

final class StreamSource: NSObject, CMIOExtensionStreamSource {
    private(set) var stream: CMIOExtensionStream!
    let deviceSource: DeviceSource
    let format: CMIOExtensionStreamFormat
    var client: CMIOExtensionClient?

    init(name: String, id: UUID, format: CMIOExtensionStreamFormat, device: DeviceSource, direction: CMIOExtensionStream.Direction) {
        self.deviceSource = device
        self.format = format
        super.init()
        stream = CMIOExtensionStream(localizedName: name, streamID: id, direction: direction, clockType: .hostTime, source: self)
    }

    var formats: [CMIOExtensionStreamFormat] { [format] }

    var availableProperties: Set<CMIOExtensionProperty> {
        var p: Set<CMIOExtensionProperty> = [.streamActiveFormatIndex, .streamFrameDuration]
        if stream.direction == .sink {
            p.formUnion([.streamSinkBufferQueueSize, .streamSinkBuffersRequiredForStartup, .streamSinkBufferUnderrunCount, .streamSinkEndOfData])
        }
        return p
    }

    func streamProperties(forProperties properties: Set<CMIOExtensionProperty>) throws -> CMIOExtensionStreamProperties {
        let p = CMIOExtensionStreamProperties(dictionary: [:])
        if properties.contains(.streamActiveFormatIndex) { p.activeFormatIndex = 0 }
        if properties.contains(.streamFrameDuration) { p.frameDuration = CMTime(value: 1, timescale: kFrameRate) }
        if properties.contains(.streamSinkBufferQueueSize) { p.sinkBufferQueueSize = 2 }
        if properties.contains(.streamSinkBuffersRequiredForStartup) { p.sinkBuffersRequiredForStartup = 1 }
        return p
    }

    func setStreamProperties(_ streamProperties: CMIOExtensionStreamProperties) throws {}

    func authorizedToStartStream(for client: CMIOExtensionClient) -> Bool {
        self.client = client
        return true
    }

    func startStream() throws {
        if stream.direction == .sink { deviceSource.startSink(client: client) } else { deviceSource.startSource() }
    }

    func stopStream() throws {
        if stream.direction == .sink { deviceSource.stopSink() } else { deviceSource.stopSource() }
    }
}

final class DeviceSource: NSObject, CMIOExtensionDeviceSource {
    private(set) var device: CMIOExtensionDevice!
    private var source: StreamSource!
    private var sink: StreamSource!
    private let queue = DispatchQueue(label: "app.cutaway.camera", qos: .userInteractive)
    private var watchers = 0
    private var sinkRunning = false
    private var sinkClient: CMIOExtensionClient?
    private var lastFed: UInt64 = 0
    private var timer: DispatchSourceTimer?
    private var placeholder: CVPixelBuffer?
    private var format: CMFormatDescription!

    init(name: String) {
        super.init()
        device = CMIOExtensionDevice(localizedName: name, deviceID: kDeviceID, legacyDeviceID: "CutawayVirtualCamera", source: self)
        var fmt: CMFormatDescription?
        CMVideoFormatDescriptionCreate(allocator: kCFAllocatorDefault, codecType: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, width: kWidth, height: kHeight, extensions: nil, formatDescriptionOut: &fmt)
        format = fmt!
        let d = CMTime(value: 1, timescale: kFrameRate)
        let streamFormat = CMIOExtensionStreamFormat(formatDescription: format, maxFrameDuration: d, minFrameDuration: d, validFrameDurations: nil)
        source = StreamSource(name: "Cutaway", id: kSourceID, format: streamFormat, device: self, direction: .source)
        sink = StreamSource(name: "Cutaway.Sink", id: kSinkID, format: streamFormat, device: self, direction: .sink)
        do {
            try device.addStream(source.stream)
            try device.addStream(sink.stream)
        } catch {
            os_log(.error, log: log, "couldn't add streams: %{public}@", error.localizedDescription)
        }
        placeholder = makePlaceholder()
    }

    var availableProperties: Set<CMIOExtensionProperty> { [.deviceTransportType, .deviceModel] }

    func deviceProperties(forProperties properties: Set<CMIOExtensionProperty>) throws -> CMIOExtensionDeviceProperties {
        let p = CMIOExtensionDeviceProperties(dictionary: [:])
        if properties.contains(.deviceTransportType) { p.transportType = kIOAudioDeviceTransportTypeVirtual }
        if properties.contains(.deviceModel) { p.model = "Cutaway Virtual Camera" }
        return p
    }

    func setDeviceProperties(_ deviceProperties: CMIOExtensionDeviceProperties) throws {}

    /// A dark card (in the camera's own format) for when Cutaway isn't sending.
    private func makePlaceholder() -> CVPixelBuffer? {
        var pb: CVPixelBuffer?
        let attrs = [kCVPixelBufferIOSurfacePropertiesKey: [:]] as CFDictionary
        CVPixelBufferCreate(kCFAllocatorDefault, Int(kWidth), Int(kHeight), kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, attrs, &pb)
        guard let pb = pb else { return nil }
        CVPixelBufferLockBaseAddress(pb, [])
        if let y = CVPixelBufferGetBaseAddressOfPlane(pb, 0) {
            memset(y, 20, CVPixelBufferGetBytesPerRowOfPlane(pb, 0) * CVPixelBufferGetHeightOfPlane(pb, 0))
        }
        if let uv = CVPixelBufferGetBaseAddressOfPlane(pb, 1) {
            memset(uv, 128, CVPixelBufferGetBytesPerRowOfPlane(pb, 1) * CVPixelBufferGetHeightOfPlane(pb, 1))
        }
        CVPixelBufferUnlockBaseAddress(pb, [])
        return pb
    }

    private func send(_ pb: CVPixelBuffer, at hostNs: UInt64) {
        var timing = CMSampleTimingInfo(duration: CMTime(value: 1, timescale: kFrameRate),
                                        presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
                                        decodeTimeStamp: .invalid)
        var sbuf: CMSampleBuffer?
        guard CMSampleBufferCreateForImageBuffer(allocator: kCFAllocatorDefault, imageBuffer: pb, dataReady: true, makeDataReadyCallback: nil, refcon: nil, formatDescription: format, sampleTiming: &timing, sampleBufferOut: &sbuf) == noErr, let sbuf = sbuf else { return }
        source.stream.send(sbuf, discontinuity: [], hostTimeInNanoseconds: hostNs)
    }

    func startSource() {
        queue.async {
            self.watchers += 1
            guard self.timer == nil else { return }
            let t = DispatchSource.makeTimerSource(flags: .strict, queue: self.queue)
            t.schedule(deadline: .now(), repeating: 1.0 / Double(kFrameRate), leeway: .milliseconds(2))
            t.setEventHandler { [weak self] in
                guard let self = self else { return }
                let now = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
                // Cutaway hasn't sent anything for half a second: show the card.
                if now - self.lastFed > 500_000_000, let p = self.placeholder {
                    self.send(p, at: now)
                }
            }
            t.resume()
            self.timer = t
        }
    }

    func stopSource() {
        queue.async {
            self.watchers = max(0, self.watchers - 1)
            if self.watchers == 0 {
                self.timer?.cancel()
                self.timer = nil
            }
        }
    }

    func startSink(client: CMIOExtensionClient?) {
        queue.async {
            self.sinkClient = client
            self.sinkRunning = true
            self.pull()
        }
    }

    func stopSink() {
        queue.async { self.sinkRunning = false }
    }

    /// Take Cutaway's next picture from the sink and show it.
    private func pull() {
        guard sinkRunning, let client = sinkClient else { return }
        sink.stream.consumeSampleBuffer(from: client) { [weak self] sbuf, seq, _, _, _ in
            guard let self = self else { return }
            self.queue.async {
                if let sbuf = sbuf, let pb = CMSampleBufferGetImageBuffer(sbuf) {
                    let now = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
                    self.lastFed = now
                    if self.watchers > 0 { self.send(pb, at: now) }
                    let out = CMIOExtensionScheduledOutput(sequenceNumber: seq, hostTimeInNanoseconds: now)
                    self.sink.stream.notifyScheduledOutputChanged(out)
                    self.pull()
                } else {
                    // Nothing waiting yet: try again shortly.
                    self.queue.asyncAfter(deadline: .now() + .milliseconds(5)) { self.pull() }
                }
            }
        }
    }
}

final class ProviderSource: NSObject, CMIOExtensionProviderSource {
    private(set) var provider: CMIOExtensionProvider!
    private var deviceSource: DeviceSource!

    init(clientQueue: DispatchQueue?) {
        super.init()
        provider = CMIOExtensionProvider(source: self, clientQueue: clientQueue)
        deviceSource = DeviceSource(name: "Cutaway")
        do {
            try provider.addDevice(deviceSource.device)
        } catch {
            os_log(.error, log: log, "couldn't add the device: %{public}@", error.localizedDescription)
        }
    }

    func connect(to client: CMIOExtensionClient) throws {}
    func disconnect(from client: CMIOExtensionClient) {}

    var availableProperties: Set<CMIOExtensionProperty> { [.providerManufacturer] }

    func providerProperties(forProperties properties: Set<CMIOExtensionProperty>) throws -> CMIOExtensionProviderProperties {
        let p = CMIOExtensionProviderProperties(dictionary: [:])
        if properties.contains(.providerManufacturer) { p.manufacturer = "Cutaway" }
        return p
    }

    func setProviderProperties(_ providerProperties: CMIOExtensionProviderProperties) throws {}
}

let providerSource = ProviderSource(clientQueue: nil)
CMIOExtensionProvider.startService(provider: providerSource.provider)
CFRunLoopRun()
