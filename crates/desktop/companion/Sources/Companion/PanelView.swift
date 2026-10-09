import AppKit
import CompanionCore
import SwiftUI
import TheuthKit

/// 149 W1 (§3.3): the card's mutable UI state the controller and the view share — the reply row's text, the
/// `screen_context` attachment P brings back, and the card's hold-to-talk (one per surface).
@MainActor
final class CardState: ObservableObject {
    let voice: HoldToTalk
    @Published var replyText = ""
    /// P's result: `files` are attached to the reply, `lines` appended as `[screen] …`.
    @Published var attached: ScreenContextReply?
    @Published var attaching = false
    /// Injected by `PanelController`: the P button and the "Stop agent X" pill.
    var onPoint: (() -> Void)?
    var onStop: (() -> Void)?

    init(store: Store) {
        voice = HoldToTalk(store: store)
        // V released: the words land in the field, which takes focus on its card (`target` = the card id).
        voice.onText = { [weak self, weak store] text in
            guard let self else { return }
            replyText = text
            if let id = voice.target { store?.ask.replyFor = id }
        }
    }
}

extension Store {
    /// 149 W1: the focused card's anatomy from the store's lists (position/count/glance + the session join).
    func anatomy(for card: Card) -> CardAnatomy {
        CardAnatomy.make(card: card, position: panel.focus + 1, count: panel.items.count, glance: panel.headline,
                         activity: activity.first { $0.id == card.item.session },
                         session: sessions.first { $0.id == card.item.session }, now: now)
    }
}

extension AttentionItem {
    /// The mark a line/status dot shows for this item.
    var markState: MarkState { kind == .finished ? .finished : (kind.needsYou ? .needsYou(kind) : .idle) }
}

/// Header → focused card → lines → Done zone → footer → receipt row (130 §4.2 order; 149 §3.3/§3.4 look).
struct PanelView: View {
    @ObservedObject var store: Store
    @ObservedObject var w1: CardState

    static let maxLines = 6

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let item = store.panel.focused {
                let card = store.card(for: item)
                let anatomy = store.anatomy(for: card)
                CardHeader(anatomy: anatomy, store: store)
                Divider()
                CardView(card: card, anatomy: anatomy, store: store, w1: w1)
                // ≤ 560 pt tall: the first lines around the focus; J/K still walk the whole list.
                let others = store.panel.others
                let shown = Array(others.prefix(Self.maxLines))
                ForEach(shown) { other in
                    Divider()
                    LineView(item: other, now: store.now).onTapGesture { store.focus(id: other.id) }
                }
                if others.count > shown.count {
                    Divider()
                    Text("+\(others.count - shown.count) more · J to walk them")
                        .font(Type.caps).foregroundColor(.tertiary).padding(.horizontal, 14).padding(.vertical, 6)
                }
            } else {
                HStack {
                    Text("Inbox").font(Type.body).fontWeight(.semibold)
                    Spacer()
                    HeaderButton(symbol: "xmark", key: "esc", hint: "Close") { store.perform(.escape) }
                }
                .padding(.horizontal, 14).padding(.vertical, 10)
                Divider()
                if store.panel.receiptFocus != nil {
                    AskItemLines(store: store) // W3: the focus is in the Done zone; the items stay as lines
                } else {
                    EmptyState(connected: store.connection == .open)
                }
            }
            DoneZone(store: store) // W3: the last 24 h of receipts, after the items
            Divider()
            footer
            if let r = store.receipt { ReceiptView(receipt: r, remaining: store.holdRemaining, delay: store.undo.delay) }
        }
        .frame(width: PanelController.width)
        .accessibilityElement(children: .contain)
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Hint("⎋", store.undo.held == nil ? "back to work" : "undo")
            Spacer()
            Text("answers wait \(Int(store.undo.delay)) s · ⎋ undoes").font(Type.caps).foregroundColor(.tertiary)
        }
        .padding(.horizontal, 14).padding(.vertical, 7)
    }
}

/// §3.4 (Revision 1 row 8): the wordmark, the headline, the sub-line.
struct EmptyState: View {
    let connected: Bool
    var body: some View {
        VStack(spacing: 10) {
            Wordmark().scaleEffect(2).frame(width: 24, height: 24)
            Text(connected ? "Nothing needs you." : "Connecting to the engine…").font(Type.headline)
            Text("Suspiciously quiet — questions and finished work land here.")
                .font(Type.meta).foregroundColor(.secondary).multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity).padding(.vertical, 28).padding(.horizontal, 20)
    }
}

/// §3.3 header row: name · position dots · ‹ J · K › · ? · ✕ esc, the glance sentence under it (Revision 1 row 7).
struct CardHeader: View {
    let anatomy: CardAnatomy
    @ObservedObject var store: Store

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 10) {
                Text(anatomy.header.name).font(Type.body).fontWeight(.semibold).lineLimit(1)
                PositionDots(position: anatomy.header.position, count: anatomy.header.count)
                Spacer(minLength: 4)
                HStack(spacing: 8) {
                    HeaderButton(symbol: "chevron.left", key: "J", hint: "Previous card") { store.perform(.previous) }
                    HeaderButton(symbol: "chevron.right", key: "K", hint: "Next card", keyFirst: true) { store.perform(.next) }
                    HeaderButton(symbol: nil, key: "?", hint: "Keyboard shortcuts") { store.perform(.legend) }
                    HeaderButton(symbol: "xmark", key: "esc", hint: "Close") { store.perform(.escape) }
                }
            }
            if !store.panel.items.isEmpty {
                Text(anatomy.header.glance).font(Type.caps).foregroundColor(.secondary)
            }
        }
        .padding(.horizontal, 14).padding(.top, 10).padding(.bottom, 8)
    }
}

/// ● ● ▬ — one per item, the focused one long. VoiceOver "card 2 of 3".
struct PositionDots: View {
    let position: Int
    let count: Int
    var body: some View {
        // ponytail: past 9 items the dots outgrow a 372 pt header; the text form takes over.
        Group {
            if count > 9 {
                Text("\(position) of \(count)").font(Type.mono).foregroundColor(.tertiary)
            } else {
                HStack(spacing: 4) {
                    ForEach(0..<max(count, 0), id: \.self) { i in
                        Capsule().fill(i == position - 1 ? Color.primary : Color.primary.opacity(0.3))
                            .frame(width: i == position - 1 ? 14 : 5, height: 5)
                    }
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("card \(position) of \(count)")
    }
}

/// An SF Symbol with its key cap, acting without activating the companion.
struct HeaderButton: View {
    let symbol: String?
    let key: String
    let hint: String
    var keyFirst = false
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            HStack(spacing: 3) {
                if keyFirst { KeyCap(key) }
                if let symbol { Image(systemName: symbol).font(Type.meta) }
                if !keyFirst { KeyCap(key) }
            }
            .foregroundColor(.secondary)
            .contentShape(Rectangle())
        }
        .buttonStyle(.borderless)
        .help(hint)
        .accessibilityLabel(hint)
    }
}

/// A key cap at `Type.capRadius`; gold when it is the recommended answer.
struct KeyCap: View {
    let key: String
    var gold = false
    init(_ key: String, gold: Bool = false) { self.key = key; self.gold = gold }
    var body: some View {
        Text(key).font(Type.mono).lineLimit(1).fixedSize()
            .padding(.horizontal, 5).frame(minWidth: 18, minHeight: 18)
            .background(RoundedRectangle(cornerRadius: Type.capRadius).fill(gold ? Color.accentColor : Color.primary.opacity(0.1)))
            .foregroundColor(gold ? .white : .primary)
    }
}

struct Hint: View {
    let key: String
    let label: String
    init(_ key: String, _ label: String) { self.key = key; self.label = label }
    var body: some View {
        HStack(spacing: 4) {
            Text(key).font(Type.mono).padding(.horizontal, 4).padding(.vertical, 1)
                .background(RoundedRectangle(cornerRadius: Type.capRadius).stroke(Color.separator, lineWidth: 1))
            Text(label).font(.caption)
        }.foregroundColor(.tertiary)
    }
}

/// The harness as an icon: `Contents/Resources/harness/<key>-{16,20,24,32}.png` (template, ink), `unknown` when the key
/// has none, the key's first letter in a rounded square when the bundle has no icons at all (bare executable).
struct HarnessIcon: View {
    var key: String
    var size: CGFloat = 20

    @MainActor private static var cache: [String: NSImage?] = [:]

    var body: some View {
        if let img = Self.image(key, size: size) {
            Image(nsImage: img).renderingMode(.template).resizable().scaledToFit()
                .frame(width: size, height: size).foregroundColor(.primary)
                .accessibilityLabel(key)
        } else {
            RoundedRectangle(cornerRadius: size * 0.25).fill(Color.primary.opacity(0.12))
                .frame(width: size, height: size)
                .overlay(Text(String(key.prefix(1)).uppercased()).font(.system(size: size * 0.55, weight: .semibold)).foregroundColor(.primary)) // native: letter scaled to the icon's geometry
                .accessibilityLabel(key)
        }
    }

    @MainActor static func image(_ key: String, size: CGFloat) -> NSImage? {
        let px = [16, 20, 24, 32].min { abs($0 - Int(size)) < abs($1 - Int(size)) } ?? 20
        let name = "\(key)-\(px)"
        if let hit = cache[name] { return hit }
        let url = Bundle.main.url(forResource: name, withExtension: "png", subdirectory: "harness")
            ?? Bundle.main.url(forResource: "unknown-\(px)", withExtension: "png", subdirectory: "harness")
        let img = url.flatMap { NSImage(contentsOf: $0) }
        img?.isTemplate = true
        cache[name] = img
        return img
    }
}

/// The focused item in full (§3.3): status line · bubbles · question · why · if-wrong · chips · options · reply row · footer.
struct CardView: View {
    let card: Card
    let anatomy: CardAnatomy
    @ObservedObject var store: Store
    @ObservedObject var w1: CardState
    @State private var note = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 7) {
                HarnessIcon(key: anatomy.status.harness, size: 16) // native: icon geometry, not a font size
                MarkView(state: card.item.markState).frame(width: 10, height: 10)
                Text("\(anatomy.status.state) · \(anatomy.status.age)" + (anatomy.status.project.map { " · \($0)" } ?? ""))
                    .font(Type.meta).foregroundColor(.secondary).lineLimit(1)
            }
            ForEach(anatomy.bubbles.indices, id: \.self) { i in BubbleView(bubble: anatomy.bubbles[i]) }
            Text(card.question).font(Type.question).lineLimit(3).fixedSize(horizontal: false, vertical: true)
            if let why = card.why, !why.isEmpty {
                Text(why).font(Type.body).foregroundColor(.secondary).lineLimit(4).fixedSize(horizontal: false, vertical: true)
            }
            if let s = anatomy.ifWrong {
                Text("If wrong: \(s)").font(Type.meta).italic().foregroundColor(.tertiary).lineLimit(2).fixedSize(horizontal: false, vertical: true)
            }
            if !card.chips.isEmpty {
                HStack(spacing: 6) {
                    ForEach(card.chips.indices, id: \.self) { i in ChipView(chip: card.chips[i]) }
                }
            }
            if let task = store.noteFor {
                NoteField(note: $note, task: task, store: store)
            } else {
                if card.item.kind == .finished, !card.options.isEmpty {
                    Text("Next steps").font(Type.caps).foregroundColor(.secondary).padding(.top, 2)
                }
                ForEach(card.options, id: \.key) { o in OptionRow(option: o) { store.pick(o, card: card) } }
                if card.moreOptions > 0 {
                    Text("+\(card.moreOptions) more — O opens the full list in theuth").font(Type.caps).foregroundColor(.tertiary)
                }
                if card.options.isEmpty && !card.clearable {
                    OptionRow(option: CardOption(key: 0, label: "Open in theuth", detail: nil, recommended: false,
                                                 action: .dismiss(id: card.item.id)), keyLabel: "O") {
                        store.perform(.openBoard)
                    }
                }
            }
            if card.replyAction("") != nil {
                ReplyRow(card: card, store: store, w1: w1, voice: w1.voice).id(card.item.id)
            }
            HStack(spacing: 8) {
                Spacer()
                if anatomy.footer.contains(.stop) {
                    FooterPill(label: "Stop agent", key: "X", tint: .failed) { w1.onStop?() }
                }
                if anatomy.footer.contains(.clear) {
                    FooterPill(label: "Clear", key: "E", tint: .secondary) { store.perform(.clear) }
                }
                if anatomy.footer.contains(.snooze) {
                    FooterPill(label: "Snooze", key: "S", tint: .secondary) { store.perform(.snooze) }
                }
            }
        }
        .padding(.horizontal, 14).padding(.top, 10).padding(.bottom, 10)
        .background(Color.primary.opacity(0.045))
        .overlay(alignment: .leading) { Rectangle().fill(Color.accentColor).frame(width: 3).opacity(card.item.kind.needsYou ? 1 : 0) }
        .onChange(of: card.item.id) { _, _ in w1.replyText = ""; w1.attached = nil } // the draft belongs to one card
    }
}

/// E6's pair: the user's request right-aligned in accent, the agent's last message left in primary.
struct BubbleView: View {
    let bubble: CardAnatomy.Bubble
    var body: some View {
        let user = bubble.side == .user
        HStack {
            if user { Spacer(minLength: 44) }
            Text(bubble.text).font(Type.meta).lineLimit(3).fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 10).padding(.vertical, 6)
                .background(
                    UnevenRoundedRectangle(topLeadingRadius: 12, bottomLeadingRadius: user ? 12 : 4,
                                           bottomTrailingRadius: user ? 4 : 12, topTrailingRadius: 12)
                        .fill(user ? Color.accentColor.opacity(0.25) : Color.primary.opacity(0.08)))
                .frame(maxWidth: 300, alignment: user ? .trailing : .leading)
            if !user { Spacer(minLength: 44) }
        }
        .accessibilityLabel(user ? "You: \(bubble.text)" : "Agent: \(bubble.text)")
    }
}

/// "1 ‹label› (Recommended) — detail →" at `Type.optionRadius`.
struct OptionRow: View {
    let option: CardOption
    var keyLabel: String? = nil
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 9) {
                KeyCap(keyLabel ?? "\(option.key)", gold: option.recommended)
                Text(option.label).font(Type.body).lineLimit(2)
                if option.recommended { Text("(Recommended)").font(Type.caps).foregroundColor(.accentColor) }
                if let d = option.detail { Text("— \(d)").font(Type.meta).foregroundColor(.tertiary).lineLimit(1) }
                Spacer()
                Image(systemName: "arrow.right").font(Type.body).foregroundColor(.secondary)
            }
            .padding(.horizontal, 10).padding(.vertical, 7)
            .background(RoundedRectangle(cornerRadius: Type.optionRadius).fill(option.recommended ? Color.accentColor.opacity(0.12) : Color.primary.opacity(0.06)))
            .overlay(RoundedRectangle(cornerRadius: Type.optionRadius).stroke(option.recommended ? Color.accentColor : Color.separator, lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(option.label)
        .accessibilityHint(keyLabel.map { "Press \($0)" } ?? "Press \(option.key)\(option.recommended ? " or Return" : "")")
    }
}

/// §3.3 reply row: [scope P] [field · R] [mic V] [arrow.up ⏎]; always visible. The field's focus mirrors `store.ask.replyFor`.
struct ReplyRow: View {
    let card: Card
    @ObservedObject var store: Store
    @ObservedObject var w1: CardState
    @ObservedObject var voice: HoldToTalk
    @FocusState private var replyFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                ReplyButton(symbol: "scope", key: "P", hint: "Point at the screen and attach it") { w1.onPoint?() }
                HStack(spacing: 6) {
                    if voice.recording || voice.finishing {
                        // Finals in primary, the open partial muted (the Ask bar's rule).
                        (Text(voice.liveText.isEmpty && voice.partial.isEmpty ? "Listening…" : voice.liveText)
                            .foregroundColor(voice.liveText.isEmpty ? .tertiary : .primary)
                            + Text(voice.liveText.isEmpty || voice.partial.isEmpty ? "" : " ")
                            + Text(voice.partial).foregroundColor(.tertiary))
                            .font(Type.body).lineLimit(2).frame(maxWidth: .infinity, alignment: .leading)
                    } else {
                        TextField(card.item.kind == .ask ? "Type your answer…" : "Reply to \(card.who)", text: $w1.replyText)
                            .textFieldStyle(.plain).font(Type.body)
                            .focused($replyFocused)
                            .onSubmit(submit)
                    }
                    KeyCap("R") // Revision 1 row 18: space is the alias
                }
                .padding(.horizontal, 8).padding(.vertical, 5)
                .background(RoundedRectangle(cornerRadius: Type.optionRadius).fill(Color.primary.opacity(0.06)))
                .overlay(RoundedRectangle(cornerRadius: Type.optionRadius).stroke(replyFocused ? Color.accentColor : Color.separator, lineWidth: 1))
                Group {
                    if voice.recording { LevelBars(levels: voice.levels) } else { HStack(spacing: 3) { Image(systemName: "mic.fill").font(Type.meta); KeyCap("V") } }
                }
                .frame(minWidth: 40, minHeight: 28)
                .background(RoundedRectangle(cornerRadius: Type.optionRadius).fill(Color.primary.opacity(0.06)))
                .contentShape(Rectangle())
                .gesture(DragGesture(minimumDistance: 0)
                    .onChanged { _ in voice.press(target: card.item.id) } // `press` is a no-op while busy
                    .onEnded { _ in voice.release() })
                .accessibilityLabel("Hold to talk")
                ReplyButton(symbol: "arrow.up", key: "⏎", hint: "Send", action: submit)
            }
            if let a = w1.attached {
                ChipView(text: "screen attached · \(a.files.count)", color: .done)
            }
            if let s = voice.status {
                Text(s).font(Type.caps).foregroundColor(voice.recording ? .failed : .tertiary)
            }
        }
        .onAppear { replyFocused = store.ask.replyFor == card.item.id }
        .onChange(of: store.ask.replyFor) { _, new in replyFocused = new == card.item.id }
        .onChange(of: replyFocused) { _, f in
            store.ask.replyFor = f ? card.item.id : (store.ask.replyFor == card.item.id ? nil : store.ask.replyFor)
        }
    }

    private func submit() {
        if let a = w1.attached { store.submitReplyWithScreen(w1.replyText, attached: a) } else { store.submitReply(w1.replyText) }
        w1.replyText = ""
        w1.attached = nil
    }
}

struct ReplyButton: View {
    let symbol: String
    let key: String
    let hint: String
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            HStack(spacing: 3) { Image(systemName: symbol).font(Type.meta); KeyCap(key) }
                .padding(.horizontal, 6).frame(minHeight: 28)
                .background(RoundedRectangle(cornerRadius: Type.optionRadius).fill(Color.primary.opacity(0.06)))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(hint)
        .accessibilityLabel(hint)
    }
}

/// The footer pills: "Stop agent X" (red) · "Clear E" · "Snooze S".
struct FooterPill: View {
    let label: String
    let key: String
    let tint: Color
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) { Text(label).font(Type.caps); KeyCap(key) }
                .foregroundColor(tint)
                .padding(.horizontal, 9).padding(.vertical, 3)
                .overlay(Capsule().stroke(tint, lineWidth: 1))
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(label) · \(key)")
    }
}

struct ChipView: View {
    let text: String
    let color: Color
    init(text: String, color: Color) { self.text = text; self.color = color }
    init(chip: Chip) {
        text = chip.text
        color = chip.ok == true ? .done : (chip.ok == false ? .failed : .secondary)
    }
    var body: some View {
        Text(text).font(Type.caps).foregroundColor(color).lineLimit(1)
            .padding(.horizontal, 7).padding(.vertical, 2)
            .overlay(Capsule().stroke(color, lineWidth: 1))
    }
}

/// The required send-back note: ⏎ sends (then the hold), ⎋ cancels.
struct NoteField: View {
    @Binding var note: String
    let task: String
    @ObservedObject var store: Store
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            TextField("Why it goes back (required)", text: $note)
                .textFieldStyle(.roundedBorder).font(Type.meta)
                .focused($focused)
                .onSubmit { store.submitNote(note); note = "" }
                .onAppear { focused = true }
            Text("⏎ send back · ⎋ cancel").font(Type.caps).foregroundColor(.tertiary)
        }
    }
}

/// A collapsed item: mark · agent · one-line title · age.
struct LineView: View {
    let item: AttentionItem
    let now: Date
    var body: some View {
        HStack(spacing: 8) {
            MarkView(state: item.markState).frame(width: 10, height: 10)
            Text(item.agent ?? item.kind.rawValue).font(Type.meta.weight(.medium))
            Text(item.title).font(Type.meta).foregroundColor(.secondary).lineLimit(1)
            Spacer()
            Text(RailModel.ago(item.waitingSince, now)).font(Type.meta.monospaced()).foregroundColor(.tertiary)
        }
        .padding(.horizontal, 14).padding(.vertical, 7)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

/// "w2 · Three tiers — sending · ⎋ undo · 2" with the countdown bar; a held Stop reads "Stopping w3 and 2 children" in red.
struct ReceiptView: View {
    let receipt: Store.Receipt
    let remaining: TimeInterval
    let delay: TimeInterval
    var width: CGFloat = PanelController.width // W3: the Ask bar is wider

    /// 149 W1: `.holding(.stopAgent)` → the children count; nil for every other receipt.
    private var stopping: Int? {
        if case .holding(let h) = receipt, case .stopAgent(_, let n) = h.action { return n }
        return nil
    }

    var body: some View {
        let tint: Color = stopping == nil ? .done : .failed
        VStack(alignment: .leading, spacing: 0) {
            Divider()
            HStack(spacing: 8) {
                switch receipt {
                case .holding(let h):
                    if let n = stopping {
                        Text("Stopping \(h.who)" + (n == 0 ? "" : n == 1 ? " and 1 child" : " and \(n) children")).font(Type.meta).foregroundColor(.failed)
                    } else {
                        Text("\(h.who) · \(h.label) — sending").font(Type.meta)
                    }
                    Spacer()
                    Text("⎋ undo · \(Int(remaining.rounded(.up)))").font(Type.meta.monospaced()).foregroundColor(.secondary)
                case .sent(let s):
                    Text("Sent · \(s)").font(Type.meta).foregroundColor(.done)
                case .undone:
                    Text("Taken back").font(Type.meta)
                case .stale:
                    Text("Already answered elsewhere").font(Type.meta).foregroundColor(.tertiary)
                case .failed(let e):
                    Text("Failed · \(e)").font(Type.meta).foregroundColor(.failed).lineLimit(1)
                case .reply(let who, let text, let session): // W3: the desk's streamed answer
                    (Text("\(who) · ").fontWeight(.semibold) + Text(text)).font(Type.meta).lineLimit(2)
                    Spacer()
                    if session != nil { Text("Open ⌘O").font(.subheadline).foregroundColor(.secondary) }
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(tint.opacity(0.12))
            .overlay(alignment: .bottomLeading) {
                if case .holding = receipt, delay > 0 {
                    Rectangle().fill(tint).frame(width: width * CGFloat(remaining / delay), height: 2)
                }
            }
        }
    }
}
