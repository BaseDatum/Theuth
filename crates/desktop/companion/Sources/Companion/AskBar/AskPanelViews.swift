import CompanionCore
import SwiftUI

/// The items as lines while the focus is in the Done zone (no card).
struct AskItemLines: View {
    @ObservedObject var store: Store
    var body: some View {
        let shown = Array(store.panel.others.prefix(PanelView.maxLines))
        ForEach(shown) { item in
            LineView(item: item, now: store.now).onTapGesture { store.focus(id: item.id) }
            Divider()
        }
        if store.panel.others.count > shown.count {
            Text("+\(store.panel.others.count - shown.count) more · K to walk them")
                .font(.subheadline).foregroundColor(.tertiary).padding(.horizontal, 12).padding(.vertical, 6)
        }
    }
}

/// "Done" — the last 24 h of asks and keystroke answers, newest first; J/K walk in, E clears, ⌘O opens.
struct DoneZone: View {
    @ObservedObject var store: Store
    static let maxRows = 4

    var body: some View {
        let all = store.ask.receipts.entries
        if !all.isEmpty {
            let focus = store.panel.receiptFocus
            // Keep the focused row on screen: a window of `maxRows` around it.
            let start = max(0, min((focus ?? 0) - DoneZone.maxRows + 1, all.count - DoneZone.maxRows))
            let rows = Array(all.enumerated().dropFirst(start).prefix(DoneZone.maxRows))
            Divider()
            HStack {
                Text("Done").font(.subheadline.weight(.semibold)).foregroundColor(.done)
                Text("· last 24 h").font(.subheadline).foregroundColor(.tertiary)
                Spacer()
                if all.count > rows.count { Text("\(all.count) · J to walk them").font(.caption).foregroundColor(.tertiary) }
            }
            .padding(.horizontal, 12).padding(.top, 7).padding(.bottom, 3)
            ForEach(rows, id: \.element.id) { i, r in
                HStack(spacing: 8) {
                    MarkView(state: .finished).frame(width: 10, height: 10)
                    Text(r.who).font(.callout.weight(.medium))
                    Text(r.text).font(.callout).foregroundColor(.secondary).lineLimit(1)
                    Spacer()
                    Text(RailModel.ago(r.at, store.now)).font(.callout.monospaced()).foregroundColor(.tertiary)
                }
                .padding(.horizontal, 12).padding(.vertical, 5)
                .background(focus == i ? Color.primary.opacity(0.045) : .clear)
                .overlay(alignment: .leading) { Rectangle().fill(Color.accentColor).frame(width: 3).opacity(focus == i ? 1 : 0) }
                .contentShape(Rectangle())
                .accessibilityElement(children: .combine)
                .accessibilityHint(r.session == nil ? "E clears" : "E clears · ⌘O opens the session")
            }
            if focus != nil {
                HStack(spacing: 10) {
                    Hint("E", "clear")
                    if store.focusedReceipt?.session != nil { Hint("⌘O", "session") }
                }
                .padding(.horizontal, 12).padding(.vertical, 5)
            }
        }
    }
}

/// R: one line, ⏎ sends through the kind's path after the hold, ⎋ cancels, empty is refused.
struct ReplyField: View {
    let card: Card
    @ObservedObject var store: Store
    @State private var text = ""
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            TextField(card.replyLabel, text: $text)
                .textFieldStyle(.roundedBorder).font(.callout)
                .focused($focused)
                .onSubmit { store.submitReply(text); text = "" }
                .onAppear { focused = true }
            Text("⏎ send · ⎋ cancel · dictation stays the Ask bar's (hold ⌃⇧Space there)").font(.caption).foregroundColor(.tertiary)
        }
    }
}
