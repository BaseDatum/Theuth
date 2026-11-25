//! `bw/<conn>/<item>[#field][:-N]` (221 §3.1): the Bitwarden half of the placeholder grammar.

use secrets::external::{ExtRef, valid_conn};

/// Which part of a Bitwarden item a reference reads or writes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BwKey {
    /// The login password (`#password`; the default on a login item).
    Password,
    /// The login username.
    Username,
    /// The current TOTP code generated from the stored seed.
    Totp,
    /// The stored TOTP seed itself (only with `expose_totp_seed`).
    TotpSeed,
    /// The item's notes (the default on a secure note).
    Notes,
    /// The login's first URI.
    Uri,
    /// The first custom field with this exact name.
    Field(String),
}

impl BwKey {
    /// Parse the text after `#`.
    pub fn parse(key: &str) -> Result<Self, String> {
        Ok(match key {
            "password" => Self::Password,
            "username" => Self::Username,
            "totp" => Self::Totp,
            "totp-seed" => Self::TotpSeed,
            "notes" => Self::Notes,
            "uri" => Self::Uri,
            _ => match key.strip_prefix("field:") {
                Some(name) if !name.is_empty() => Self::Field(name.to_owned()),
                _ => {
                    return Err(format!(
                        "unknown Bitwarden field #{key}: use password, username, totp, totp-seed, \
                         notes, uri or field:<name>"
                    ));
                }
            },
        })
    }

    /// The text after `#`, the inverse of [`BwKey::parse`].
    pub fn as_key(&self) -> String {
        match self {
            Self::Password => "password".into(),
            Self::Username => "username".into(),
            Self::Totp => "totp".into(),
            Self::TotpSeed => "totp-seed".into(),
            Self::Notes => "notes".into(),
            Self::Uri => "uri".into(),
            Self::Field(name) => format!("field:{name}"),
        }
    }

    /// The key of an [`ExtRef`]; `None` = the item's default field.
    pub fn of(r: &ExtRef) -> Result<Option<Self>, String> {
        r.key.as_deref().map(Self::parse).transpose()
    }
}

/// A canonical lowercase UUID (`CipherId` text).
pub fn valid_item_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}

/// Parse a Bitwarden reference: the placeholder body `bw/<conn>/<item>[#field][:-N]`, with or
/// without the surrounding `{{secret:` … `}}`. Errors are model-facing.
pub fn parse_ref(text: &str) -> Result<ExtRef, String> {
    let body = text
        .strip_prefix("{{secret:")
        .and_then(|b| b.strip_suffix("}}"))
        .unwrap_or(text);
    let rest = body
        .strip_prefix("bw/")
        .ok_or_else(|| format!("{body} is not a Bitwarden reference (bw/<connection>/<item>)"))?;
    let (conn, rest) = rest
        .split_once('/')
        .ok_or_else(|| format!("{body}: expected bw/<connection>/<item>"))?;
    if !valid_conn(conn) {
        return Err(format!(
            "{body}: invalid connection name {conn:?} ([a-z0-9-], 1 to 32 characters)"
        ));
    }
    let (rest, back) = split_back(rest);
    let (id, key) = match rest.split_once('#') {
        Some((id, key)) => (id, Some(BwKey::parse(key)?)),
        None => (rest, None),
    };
    if !valid_item_id(id) {
        return Err(format!(
            "{body}: the item must be a lowercase item id (a UUID such as \
             2f1c0a8e-6b1d-4c49-9d7e-0a1b2c3d4e5f), not {id:?}"
        ));
    }
    if back > 0 && !matches!(key, None | Some(BwKey::Password)) {
        return Err(format!(
            "{body}: only #password has history in Bitwarden (:-N)"
        ));
    }
    Ok(ExtRef {
        conn: conn.to_owned(),
        id: id.to_owned(),
        key: key.map(|k| k.as_key()),
        back,
    })
}

/// The placeholder body of `r` (`bw/<conn>/<item>[#key][:-N]`).
pub fn format_ref(r: &ExtRef) -> String {
    let mut s = format!("bw/{}/{}", r.conn, r.id);
    if let Some(k) = &r.key {
        s.push('#');
        s.push_str(k);
    }
    if r.back > 0 {
        s.push_str(&format!(":-{}", r.back));
    }
    s
}

/// Split a trailing `:-N` (N ∈ 1..=4) off `s`.
fn split_back(s: &str) -> (&str, u8) {
    if let Some((head, n)) = s.rsplit_once(":-")
        && let [d @ b'1'..=b'4'] = n.as_bytes()
    {
        return (head, d - b'0');
    }
    (s, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "2f1c0a8e-6b1d-4c49-9d7e-0a1b2c3d4e5f";

    fn r(key: Option<&str>, back: u8) -> ExtRef {
        ExtRef {
            conn: "personal".into(),
            id: ID.into(),
            key: key.map(str::to_owned),
            back,
        }
    }

    #[test]
    fn parses_every_field_and_round_trips() {
        for (key, want) in [
            ("password", BwKey::Password),
            ("username", BwKey::Username),
            ("totp", BwKey::Totp),
            ("totp-seed", BwKey::TotpSeed),
            ("notes", BwKey::Notes),
            ("uri", BwKey::Uri),
            ("field:api token", BwKey::Field("api token".into())),
            ("field:a:b", BwKey::Field("a:b".into())),
        ] {
            let text = format!("bw/personal/{ID}#{key}");
            let got = parse_ref(&text).unwrap();
            assert_eq!(got, r(Some(key), 0), "{text}");
            assert_eq!(BwKey::of(&got).unwrap(), Some(want));
            assert_eq!(format_ref(&got), text);
        }
    }

    #[test]
    fn default_field_history_and_placeholder_wrapping() {
        assert_eq!(parse_ref(&format!("bw/personal/{ID}")).unwrap(), r(None, 0));
        assert_eq!(
            parse_ref(&format!("{{{{secret:bw/personal/{ID}#password:-2}}}}")).unwrap(),
            r(Some("password"), 2)
        );
        assert_eq!(
            parse_ref(&format!("bw/personal/{ID}:-4")).unwrap(),
            r(None, 4)
        );
        // `:-5` is not a history suffix, so it stays in the field name.
        assert_eq!(
            parse_ref(&format!("bw/personal/{ID}#field:x:-5")).unwrap(),
            r(Some("field:x:-5"), 0)
        );
    }

    #[test]
    fn refuses_bad_references() {
        for bad in [
            format!("aws/personal/{ID}"),
            format!("bw/Personal/{ID}"),
            format!("bw/personal/{}", ID.to_uppercase()),
            "bw/personal/my-login".to_owned(),
            format!("bw/personal/{ID}#secret"),
            format!("bw/personal/{ID}#field:"),
            format!("bw/personal/{ID}#username:-1"),
            format!("bw/personal/{ID}#field:x:-1"),
            "bw/personal".to_owned(),
        ] {
            assert!(parse_ref(&bad).is_err(), "{bad} parsed");
        }
    }
}
