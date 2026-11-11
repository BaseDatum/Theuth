import AppKit
import SwiftUI

extension Flags {
    /// 189 F2: `--probe-markdown` — render a fixed answer through `AnswerMarkdown` off-screen, report, exit (`MarkdownProbe`).
    static let probeMarkdown = args.contains("--probe-markdown")
}

/// 189 F2: the gate that the markdown library's resource bundles load in the shipped app (from Contents/Resources, with
/// no build tree). `main.swift` runs it before the app delegate exists: no engine connection, no hotkeys, and the one
/// window is never ordered front (it only lets `.task` run and layout happen; it drives no fade, so the sample renders
/// without it). The sample reaches the views that read `Bundle.module` — bullets, the quote bar, code-block and table
/// chrome, highlight.js; a bundle the accessor cannot find traps there (exit 133, the accessor's message on stderr).
/// Reports to companion.log and stderr: each loaded package `*.bundle` and whether it sits in Contents/Resources, the
/// rendered size, the PNG; exit 0 on success, 1 with the reason otherwise (SwiftStreamingMarkdown's bundle never loaded,
/// the sample did not lay out).
@MainActor
enum MarkdownProbe {
    static let png = URL(fileURLWithPath: "/tmp/189-probe-markdown.png")

    static let sample = """
    # Status

    The desk is **done**, the build is *green* and `crates/rpc` passed; see [the board](https://theuth.io).

    - handed to w3
    - told w2 to wait

    1. first
    2. second

    > a quote from the desk

    ```swift
    let answer = 42
    ```

    | lane | state |
    |---|---|
    | w2 | waiting |
    | w3 | done |

    Half-streamed: **bold
    """

    static func run() -> Never {
        let width = AnswerCard.width
        let host = NSHostingView(rootView: AnswerMarkdown(text: sample, animate: false).padding(16).frame(width: width)
            .background(Color(nsColor: .windowBackgroundColor))) // label colours need the appearance's own background in the PNG
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: width, height: 800), styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = host
        // The parse is async and highlight.js colours the code after it: wait until the size has held for 0.5 s.
        var size = NSSize.zero, since = Date()
        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.1))
            host.layoutSubtreeIfNeeded()
            let s = host.fittingSize
            if s != size { size = s; since = Date() } else if size.height > 200, Date().timeIntervalSince(since) > 0.5 { break }
        }
        guard size.height > 200 else { fail("rendered height \(size.height) pt after 10 s: the sample did not lay out") }
        window.setContentSize(NSSize(width: width, height: size.height))
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.3)) // one more pass at the final size
        guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { fail("no bitmap for \(host.bounds)") }
        host.cacheDisplay(in: host.bounds, to: rep)

        // The packages' resource bundles (`Package_Target.bundle`); the system's own are not ours to report.
        let resources = Bundle.main.resourceURL?.standardizedFileURL.path ?? ""
        let bundles = Bundle.allBundles.map { $0.bundleURL.standardizedFileURL.path }
            .filter { $0.hasSuffix(".bundle") && !$0.hasPrefix("/System/") }.sorted()
        for b in bundles {
            let inside = !resources.isEmpty && b.hasPrefix(resources + "/")
            Log.write("probe-markdown: loaded \(b) (\(inside ? "Contents/Resources" : "outside the app's Contents/Resources"))")
        }
        guard bundles.contains(where: { ($0 as NSString).lastPathComponent.hasPrefix("SwiftStreamingMarkdown_") }) else {
            fail("SwiftStreamingMarkdown's resource bundle was never loaded: the sample did not reach its Bundle.module")
        }
        do { try rep.representation(using: .png, properties: [:])?.write(to: png) } catch { fail("writing \(png.path): \(error)") }
        Log.write("probe-markdown: rendered \(Int(host.bounds.width))×\(Int(host.bounds.height)) pt (\(rep.pixelsWide)×\(rep.pixelsHigh) px) → \(png.path)")
        Log.write("probe-markdown: ok")
        exit(0)
    }

    private static func fail(_ why: String) -> Never {
        Log.write("probe-markdown: FAILED — \(why)")
        exit(1)
    }
}
