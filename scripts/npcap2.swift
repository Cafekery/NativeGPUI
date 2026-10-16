// npcap2 <windowID> <seconds> <out.txt> <rx> <ry> <rw> <rh> [px py]
// Per composited frame: presentation time, status, FNV hash of a device-pixel region, and the
// mean colour of an optional 5x5 point. Lines: "F t=<s> s=<status> h=<hex> p=<r,g,b>"
import AppKit
import CoreGraphics
import Foundation
import ScreenCaptureKit

_ = NSApplication.shared
let args = CommandLine.arguments
guard args.count >= 8, let wid = UInt32(args[1]), let secs = Double(args[2]),
      let rx = Int(args[4]), let ry = Int(args[5]), let rw = Int(args[6]), let rh = Int(args[7]) else {
    FileHandle.standardError.write("usage: npcap2 <windowID> <seconds> <out.txt> <rx> <ry> <rw> <rh> [px py]\n".data(using: .utf8)!)
    exit(2)
}
let outPath = args[3]
let px = args.count >= 10 ? Int(args[8]) : nil
let py = args.count >= 10 ? Int(args[9]) : nil
let scx = args.count >= 12 ? Int(args[10]) : nil
let scw = args.count >= 12 ? Int(args[11]) : nil

final class Sink: NSObject, SCStreamOutput {
    let rx: Int, ry: Int, rw: Int, rh: Int, px: Int?, py: Int?, scx: Int?, scw: Int?
    var prevRows: [UInt64] = []
    init(rx: Int, ry: Int, rw: Int, rh: Int, px: Int?, py: Int?, scx: Int?, scw: Int?) { self.rx = rx; self.ry = ry; self.rw = rw; self.rh = rh; self.px = px; self.py = py; self.scx = scx; self.scw = scw }
    var lines: [String] = []
    var byStatus: [Int: Int] = [:]
    let lock = NSLock()
    func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen else { return }
        lock.lock(); defer { lock.unlock() }
        var status = -1
        if let attach = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
           let raw = attach.first?[.status] as? Int { status = raw }
        byStatus[status, default: 0] += 1
        let t = CMSampleBufferGetPresentationTimeStamp(sb).seconds
        guard sb.isValid, let buf = sb.imageBuffer else { lines.append(String(format: "F t=%.6f s=%d h=- p=-", t, status)); return }
        CVPixelBufferLockBaseAddress(buf, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(buf, .readOnly) }
        let w = CVPixelBufferGetWidth(buf), h = CVPixelBufferGetHeight(buf)
        let stride = CVPixelBufferGetBytesPerRow(buf)
        guard let base = CVPixelBufferGetBaseAddress(buf) else { return }
        let bytes = base.assumingMemoryBound(to: UInt8.self)
        var hash: UInt64 = 0xcbf29ce484222325
        let x0 = max(0, rx), x1 = min(w, rx + rw), y0 = max(0, ry), y1 = min(h, ry + rh)
        if x0 < x1 && y0 < y1 {
            for y in stride_(y0, y1, 2) {
                let row = bytes.advanced(by: y * stride)
                var x = x0
                while x < x1 {
                    let v = UInt64(row[x * 4]) | UInt64(row[x * 4 + 1]) << 8 | UInt64(row[x * 4 + 2]) << 16
                    hash = (hash ^ v) &* 0x100000001b3
                    x += 2
                }
            }
        }
        var p = "-"
        if let cx = px, let cy = py, cx >= 2, cy >= 2, cx + 2 < w, cy + 2 < h {
            var sr = 0, sg = 0, sb2 = 0
            for y in (cy - 2)...(cy + 2) {
                let row = bytes.advanced(by: y * stride)
                for x in (cx - 2)...(cx + 2) { sb2 += Int(row[x * 4]); sg += Int(row[x * 4 + 1]); sr += Int(row[x * 4 + 2]) }
            }
            p = String(format: "%.1f,%.1f,%.1f", Double(sr) / 25, Double(sg) / 25, Double(sb2) / 25)
        }
        var shift = "?"
        if let cx = scx, let cw = scw, cx + cw <= w {
            var rows = [UInt64](repeating: 0, count: h)
            for y in 0..<h {
                let row = bytes.advanced(by: y * stride)
                var rh2: UInt64 = 0xcbf29ce484222325
                var x = cx
                while x < cx + cw {
                    let v = UInt64(row[x * 4]) | UInt64(row[x * 4 + 1]) << 8 | UInt64(row[x * 4 + 2]) << 16
                    rh2 = (rh2 ^ v) &* 0x100000001b3
                    x += 2
                }
                rows[y] = rh2
            }
            if prevRows.count == h {
                // rows that carry structure: differ from the row above
                var idx: [Int] = []
                for y in max(120, ry)..<min(h - 120, ry + rh) where rows[y] != rows[y - 1] { idx.append(y) }
                if idx.count >= 8 {
                    var best = 0, bestN = -1
                    for s in -100...100 {
                        var n = 0
                        for y in idx where rows[y] == prevRows[y + s] { n += 1 }
                        if n > bestN || (n == bestN && abs(s) < abs(best)) { bestN = n; best = s }
                    }
                    shift = String(format: "%d/%.2f", best, Double(bestN) / Double(idx.count))
                }
            }
            prevRows = rows
        }
        lines.append(String(format: "F t=%.6f s=%d h=%016llx p=%@ d=%@", t, status, hash, p, shift))
    }
    func stride_(_ a: Int, _ b: Int, _ s: Int) -> StrideTo<Int> { Swift.stride(from: a, to: b, by: s) }
}

let sem = DispatchSemaphore(value: 0)
var exitCode: Int32 = 0
Task {
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        guard let win = content.windows.first(where: { $0.windowID == wid }) else {
            FileHandle.standardError.write("no window \(wid)\n".data(using: .utf8)!); exitCode = 3; sem.signal(); return
        }
        let filter = SCContentFilter(desktopIndependentWindow: win)
        let cfg = SCStreamConfiguration()
        cfg.width = Int(filter.contentRect.width * CGFloat(filter.pointPixelScale))
        cfg.height = Int(filter.contentRect.height * CGFloat(filter.pointPixelScale))
        cfg.pixelFormat = kCVPixelFormatType_32BGRA
        cfg.colorSpaceName = CGColorSpace.sRGB
        let hz = Int32(ProcessInfo.processInfo.environment["NPCAP_HZ"] ?? "120") ?? 120
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: hz)
        cfg.queueDepth = 8
        cfg.showsCursor = false
        cfg.capturesAudio = false
        cfg.scalesToFit = false
        let sink = Sink(rx: rx, ry: ry, rw: rw, rh: rh, px: px, py: py, scx: scx, scw: scw)
        let stream = SCStream(filter: filter, configuration: cfg, delegate: nil)
        try stream.addStreamOutput(sink, type: .screen, sampleHandlerQueue: DispatchQueue(label: "npcap2", qos: .userInteractive))
        try await stream.startCapture()
        print("NPCAP-START hz=\(hz) w=\(cfg.width) h=\(cfg.height) scale=\(filter.pointPixelScale) rect=\(filter.contentRect)")
        fflush(stdout)
        try await Task.sleep(nanoseconds: UInt64(secs * 1e9))
        try? await stream.stopCapture()
        sink.lock.lock()
        let text = sink.lines.joined(separator: "\n")
        let status = sink.byStatus.sorted { $0.key < $1.key }.map { "\($0.key):\($0.value)" }.joined(separator: ",")
        sink.lock.unlock()
        try text.write(toFile: outPath, atomically: true, encoding: .utf8)
        print("NPCAP-DONE lines=\(sink.lines.count) status=\(status)")
        fflush(stdout)
    } catch {
        FileHandle.standardError.write("npcap2 error: \(error)\n".data(using: .utf8)!); exitCode = 4
    }
    sem.signal()
}
sem.wait()
exit(exitCode)
