import Foundation

/// 189 D1: the one owner of the connection's `set_event_focus` set. The engine keeps ONE set per connection and each call
/// REPLACES it; while a set exists it drops `TextDelta`/`ReasoningDelta`/`ToolCallProgress`/`VisionProgress` for every
/// session outside it (`crates/rpc/src/api.rs` `set_event_focus`, `is_delta`). Every surface names its sessions as a claim;
/// the engine gets the union. Sends are serialized and latest-wins: one in flight, and when it lands the union as it is
/// *then* goes out if it differs — an older union never overwrites a newer one. Pure; `Store.claimFocus` drives the RPC.
public struct FocusCoordinator: Equatable, Sendable {
    public enum Claim: String, Equatable, Sendable, CaseIterable {
        /// 155 §13.5: the focused card's session while the Attention panel is open.
        case panel
        /// The desk while an Ask is collecting or its answer card is up.
        case ask
        /// The desk while a voice-key / command-key reply is collecting.
        case point
    }

    public private(set) var claims: [Claim: [String]] = [:]
    /// The union being sent; `nil` when no send is in flight.
    public private(set) var inFlight: [String]?
    /// The union the last send carried (delivered or refused: an engine before 129 refuses every time); `nil` = none yet.
    public private(set) var sent: [String]?
    /// A reconnect or `PendingPrompts` asked for the union again even if it did not change.
    private var force = false

    public init() {}

    /// Every claimed session once, sorted (a stable value to compare).
    public var union: [String] { Array(Set(claims.values.joined())).sorted() }

    /// Set `claim`'s sessions (`[]` withdraws it). Answers the union to send now, or `nil` (unchanged, or one in flight).
    public mutating func claim(_ claim: Claim, _ sessions: [String]) -> [String]? {
        claims[claim] = sessions.isEmpty ? nil : sessions
        return next()
    }

    /// A connection opened or a (re)subscription's `PendingPrompts` arrived: the engine may hold another set — send ours.
    public mutating func resend() -> [String]? {
        force = true
        return next()
    }

    /// The send of `union` came back (ok or refused). Answers the next union to send, if the claims moved meanwhile.
    public mutating func finished(_ union: [String]) -> [String]? {
        inFlight = nil
        sent = union
        return next()
    }

    private mutating func next() -> [String]? {
        guard inFlight == nil else { return nil }
        let u = union
        guard force || u != sent else { return nil }
        force = false
        inFlight = u
        return u
    }
}
