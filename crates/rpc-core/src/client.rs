/// 130 E4: the client a connection declared with `X-Theuth-Client` on the
/// WebSocket upgrade — `desktop | tui | ios | companion`, `desktop` when the
/// header is absent or unknown. the RPC transport puts it in every admitted request's
/// extensions beside the peer, so each call on the socket can stamp
/// `ActivityEntry.actor = user@<client>` ([`Client::actor`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client(pub &'static str);

impl Client {
    /// The header name.
    pub const HEADER: &'static str = "x-theuth-client";
    /// What an absent or unknown header means.
    pub const DEFAULT: Client = Client("desktop");
    const KNOWN: [&'static str; 4] = ["desktop", "tui", "ios", "companion"];

    /// The header's value when it names a known client, else [`Self::DEFAULT`].
    pub fn parse(value: Option<&str>) -> Self {
        value
            .map(str::trim)
            .and_then(|v| Self::KNOWN.iter().find(|k| k.eq_ignore_ascii_case(v)))
            .map_or(Self::DEFAULT, |k| Client(k))
    }

    /// `user@<client>` for the connection behind `ext` (`user@desktop` when
    /// the call did not come over the socket, e.g. the in-process TUI).
    pub fn actor(ext: &jsonrpsee::Extensions) -> String {
        format!("user@{}", ext.get::<Client>().unwrap_or(&Self::DEFAULT).0)
    }
}

/// The local connection's `conn=` id in every admitted request's extensions, so
/// `client_log` lines name the socket the `rpc: ws open` line announced (176 C2).
#[derive(Clone, Copy, Debug)]
pub struct ConnId(pub u64);
