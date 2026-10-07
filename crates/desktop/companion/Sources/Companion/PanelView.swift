import CompanionCore
import SwiftUI
import TheuthKit

/// Header → focused card → lines → key hints → receipt row (130 §4.2, fixed order).
struct PanelView: View {
    @ObservedObject var store: Store

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider().overlay(Color.rule.opacity(0.6))
            if let item = store.panel.focused {
                CardView(card: store.card(for: item), store: store)
                ForEach(store.panel.others) { other in
                    Divider().overlay(Color.rule.opacity(0.4))
                    LineView(item: other, now: store.now).onTapGesture { store.focus(id: other.id) }
                }
            } else {
                Text(store.connection == .open ? "Nothing needs you." : "Connecting to the engine…")
                    .font(.system(size: 12.5)).foregroundColor(.ink2)
                    .frame(maxWidth: .infinity).padding(.vertical, 26)
            }
            Divider().overlay(Color.rule.opacity(0.6))
            footer
            if let r = store.receipt { ReceiptView(receipt: r, remaining: store.holdRemaining, delay: store.undo.delay) }
        }
        .frame(width: PanelController.width)
        .foregroundColor(.ink)
        .accessibilityElement(children: .contain)
    }

    private var header: some View {
        HStack(spacing: 8) {
            if store.rail.needsYouCount > 0 { Circle().fill(Color.gold).frame(width: 7, height: 7) }
            headline.font(.system(size: 11.5))
            Spacer()
            Text(store.panel.position).font(.system(size: 11.5, design: .monospaced)).foregroundColor(.sand)
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
    }

    /// The needs-you count is the only gold text in the header.
    private var headline: Text {
        let parts = store.panel.headline.components(separatedBy: " · ")
        let gold = store.rail.needsYouCount > 0
        var t = Text(parts[0]).foregroundColor(gold ? .gold : .ink2).fontWeight(gold ? .semibold : .regular)
        for p in parts.dropFirst() { t = t + Text(" · \(p)").foregroundColor(.ink2) }
        return t
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Hint("J K", "move"); Hint("E", "clear"); Hint("⎋", store.undo.held == nil ? "back to work" : "undo")
            Spacer()
            Text("answers wait \(Int(store.undo.delay)) s · ⎋ undoes").font(.system(size: 10.5)).foregroundColor(.sand)
        }
        .padding(.horizontal, 12).padding(.vertical, 7)
    }
}

struct Hint: View {
    let key: String
    let label: String
    init(_ key: String, _ label: String) { self.key = key; self.label = label }
    var body: some View {
        HStack(spacing: 4) {
            Text(key).font(.system(size: 10, design: .monospaced)).padding(.horizontal, 4).padding(.vertical, 1)
                .background(RoundedRectangle(cornerRadius: 3).stroke(Color.rule, lineWidth: 1))
            Text(label).font(.system(size: 10.5))
        }.foregroundColor(.sand)
    }
}

/// The focused item in full, in the order a person decides.
struct CardView: View {
    let card: Card
    @ObservedObject var store: Store
    @State private var note = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 7) {
                MarkView(state: card.item.kind == .finished ? .finished : .needsYou(card.item.kind)).frame(width: 10, height: 10)
                Text(card.who).font(.system(size: 11.5, weight: .semibold))
                if let m = card.model { Text("· \(m)").font(.system(size: 11.5)).foregroundColor(.ink2) }
                if card.question != card.item.title { Text("· \(card.item.title)").font(.system(size: 11.5)).foregroundColor(.ink2).lineLimit(1) }
                Spacer()
                Text(card.item.kind == .finished ? "finished \(RailModel.ago(card.item.waitingSince, store.now)) ago"
                     : "waiting \(RailModel.ago(card.item.waitingSince, store.now))")
                    .font(.system(size: 11.5, design: .monospaced)).foregroundColor(.sand)
            }
            Text(card.question).font(.system(size: 13.5, weight: .semibold)).lineLimit(3).fixedSize(horizontal: false, vertical: true)
            if let why = card.why, !why.isEmpty {
                Text(why).font(.system(size: 11.5)).foregroundColor(.ink2).lineLimit(4).fixedSize(horizontal: false, vertical: true)
            }
            if !card.chips.isEmpty {
                HStack(spacing: 6) {
                    ForEach(card.chips.indices, id: \.self) { i in ChipView(chip: card.chips[i]) }
                }
            }
            if let task = store.noteFor {
                NoteField(note: $note, task: task, store: store)
            } else {
                ForEach(card.options, id: \.key) { o in OptionRow(option: o) { store.pick(o, card: card) } }
                if card.moreOptions > 0 {
                    Text("+\(card.moreOptions) more — O opens the full list in theuth").font(.system(size: 10.5)).foregroundColor(.sand)
                }
                if card.options.isEmpty && !card.clearable {
                    OptionRow(option: CardOption(key: 0, label: "Open in theuth", detail: nil, recommended: false,
                                                 action: .dismiss(id: card.item.id)), keyLabel: "O") {
                        store.perform(.openBoard)
                    }
                }
            }
            HStack(spacing: 10) {
                Hint("S", "snooze 30 m")
                if card.clearable { Hint("E", "clear") }
                if card.item.kind == .decision { Hint("O", "board") }
                if card.item.kind == .review || card.item.kind == .finished { Hint("D", "diff") }
                if card.sessionURL != nil { Hint("⌘O", "session") }
            }
        }
        .padding(.horizontal, 12).padding(.top, 11).padding(.bottom, 10)
        .background(Color.ink.opacity(0.045))
        .overlay(alignment: .leading) { Rectangle().fill(Color.gold).frame(width: 3).opacity(card.item.kind.needsYou ? 1 : 0) }
    }
}

struct OptionRow: View {
    let option: CardOption
    var keyLabel: String? = nil
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 9) {
                Text(keyLabel ?? "\(option.key)")
                    .font(.system(size: 10.5, design: .monospaced))
                    .frame(minWidth: 18, minHeight: 18)
                    .background(RoundedRectangle(cornerRadius: 4).fill(option.recommended ? Color.gold : Color.ink.opacity(0.08)))
                    .foregroundColor(option.recommended ? .surface : .ink)
                Text(option.label).font(.system(size: 12.5)).lineLimit(2)
                if let d = option.detail { Text("— \(d)").font(.system(size: 11)).foregroundColor(.sand).lineLimit(1) }
                Spacer()
                if option.recommended { Text("recommended · ⏎").font(.system(size: 10)).foregroundColor(.gold) }
            }
            .padding(.horizontal, 8).padding(.vertical, 6)
            .background(RoundedRectangle(cornerRadius: 9).fill(option.recommended ? Color.gold.opacity(0.12) : Color.ink.opacity(0.06)))
            .overlay(RoundedRectangle(cornerRadius: 9).stroke(option.recommended ? Color.gold : Color.rule.opacity(0.6), lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(option.label)
        .accessibilityHint(keyLabel.map { "Press \($0)" } ?? "Press \(option.key)\(option.recommended ? " or Return" : "")")
    }
}

struct ChipView: View {
    let chip: Chip
    var body: some View {
        let c: Color = chip.ok == true ? .malachite : (chip.ok == false ? .carnelian : .ink2)
        Text(chip.text).font(.system(size: 10.5)).foregroundColor(c).lineLimit(1)
            .padding(.horizontal, 7).padding(.vertical, 2)
            .overlay(Capsule().stroke(c, lineWidth: 1))
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
                .textFieldStyle(.roundedBorder).font(.system(size: 12.5))
                .focused($focused)
                .onSubmit { store.submitNote(note); note = "" }
                .onAppear { focused = true }
            Text("⏎ send back · ⎋ cancel").font(.system(size: 10.5)).foregroundColor(.sand)
        }
    }
}

/// A collapsed item: mark · agent · one-line title · age.
struct LineView: View {
    let item: AttentionItem
    let now: Date
    var body: some View {
        HStack(spacing: 8) {
            MarkView(state: item.kind == .finished ? .finished : (item.kind.needsYou ? .needsYou(item.kind) : .idle)).frame(width: 10, height: 10)
            Text(item.agent ?? item.kind.rawValue).font(.system(size: 12, weight: .medium))
            Text(item.title).font(.system(size: 12)).foregroundColor(.ink2).lineLimit(1)
            Spacer()
            Text(RailModel.ago(item.waitingSince, now)).font(.system(size: 12, design: .monospaced)).foregroundColor(.sand)
        }
        .padding(.horizontal, 12).padding(.vertical, 7)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

/// "w2 · Three tiers — sending · ⎋ undo · 2" with the countdown bar.
struct ReceiptView: View {
    let receipt: Store.Receipt
    let remaining: TimeInterval
    let delay: TimeInterval

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Divider().overlay(Color.rule.opacity(0.6))
            HStack(spacing: 8) {
                switch receipt {
                case .holding(let h):
                    Text("\(h.who) · \(h.label) — sending").font(.system(size: 12))
                    Spacer()
                    Text("⎋ undo · \(Int(remaining.rounded(.up)))").font(.system(size: 12, design: .monospaced)).foregroundColor(.ink2)
                case .sent(let s):
                    Text("Sent · \(s)").font(.system(size: 12)).foregroundColor(.malachite)
                case .undone:
                    Text("Taken back").font(.system(size: 12))
                case .stale:
                    Text("Already answered elsewhere").font(.system(size: 12)).foregroundColor(.sand)
                case .failed(let e):
                    Text("Failed · \(e)").font(.system(size: 12)).foregroundColor(.carnelian).lineLimit(1)
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(Color.malachite.opacity(0.12))
            .overlay(alignment: .bottomLeading) {
                if case .holding = receipt, delay > 0 {
                    Rectangle().fill(Color.malachite).frame(width: PanelController.width * CGFloat(remaining / delay), height: 2)
                }
            }
        }
    }
}
