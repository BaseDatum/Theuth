//! Field mapping between theuth's references (`#password`, `#field:<name>`, …) and the SDK's
//! decrypted views (221 §3.1, §5.5, §5.6). Pure functions over SDK types; no I/O.

use bitwarden_vault::{
    CipherCreateRequest, CipherEditRequest, CipherRepromptType, CipherType, CipherView,
    CipherViewType, FieldType, FieldView, LoginUriView, LoginView, SecureNoteType, SecureNoteView,
};
use chrono::{DateTime, SecondsFormat, Utc};
use secrets::external::{BwField, BwKind, ExtError, ExtVersion};
use zeroize::Zeroizing;

use crate::config::BwConfig;
use crate::refs::BwKey;

/// `LoginLinkedIdType` (`linked_id.rs`): a linked custom field pointing at the username.
const LINKED_USERNAME: u32 = 100;
/// … and at the password.
const LINKED_PASSWORD: u32 = 101;

/// `ExtEntry.kind` for a cipher type.
pub fn kind_name(t: CipherType) -> &'static str {
    match t {
        CipherType::Login => "login",
        CipherType::SecureNote => "secure_note",
        CipherType::Card => "card",
        CipherType::Identity => "identity",
        CipherType::SshKey => "ssh_key",
        CipherType::BankAccount => "bank_account",
        CipherType::DriversLicense => "drivers_license",
        CipherType::Passport => "passport",
    }
}

/// A revision date as `ExtVersion` (Bitwarden: one version, id = revision date).
pub fn version(revision: DateTime<Utc>) -> ExtVersion {
    let id = revision.to_rfc3339_opts(SecondsFormat::AutoSi, true);
    ExtVersion {
        changed: Some(id.clone()),
        id,
        stages: Vec::new(),
    }
}

/// Whether `expected` names the revision `current` (compared as instants, not text).
pub fn same_version(expected: &ExtVersion, current: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(&expected.id).is_ok_and(|t| t == current)
}

/// The referenceable keys of an item (names only), for `describe`.
pub fn field_keys(v: &CipherView) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(l) = &v.login {
        if l.password.is_some() {
            keys.push("password".to_owned());
        }
        if l.username.is_some() {
            keys.push("username".to_owned());
        }
        if l.totp.is_some() {
            keys.push("totp".to_owned());
        }
        if l.uris
            .as_ref()
            .is_some_and(|u| u.iter().any(|u| u.uri.is_some()))
        {
            keys.push("uri".to_owned());
        }
    }
    if v.notes.is_some() {
        keys.push("notes".to_owned());
    }
    for f in v.fields.iter().flatten() {
        if let Some(name) = &f.name {
            let key = format!("field:{name}");
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

fn supported(v: &CipherView) -> Result<(), ExtError> {
    match v.r#type {
        CipherType::Login | CipherType::SecureNote => Ok(()),
        t => Err(ExtError::Unsupported(format!(
            "{} items cannot be referenced; only logins and secure notes",
            kind_name(t)
        ))),
    }
}

/// The key a reference without `#…` reads: the login password, or a secure note's notes.
fn default_key(v: &CipherView) -> BwKey {
    match v.r#type {
        CipherType::SecureNote => BwKey::Notes,
        _ => BwKey::Password,
    }
}

/// The value a reference resolves to (221 §3.1). `conn` names the connection in errors.
pub fn read_field(
    v: &CipherView,
    key: Option<&BwKey>,
    back: u8,
    cfg: &BwConfig,
    conn: &str,
    now: Option<DateTime<Utc>>,
) -> Result<Zeroizing<String>, ExtError> {
    supported(v)?;
    let key = key.cloned().unwrap_or_else(|| default_key(v));
    let login = || v.login.as_ref().ok_or(ExtError::NotFound);
    let some = |o: Option<&String>| {
        o.map(|s| Zeroizing::new(s.clone()))
            .ok_or(ExtError::NotFound)
    };
    if back > 0 {
        if key != BwKey::Password {
            return Err(ExtError::Unsupported(
                "only #password has history in Bitwarden".into(),
            ));
        }
        if !cfg.expose_password_history {
            return Err(ExtError::Denied(format!(
                "bw/{conn} does not expose password history; Settings › Secrets › {conn}"
            )));
        }
        // Newest first, as the SDK prepends (`update_password_history`).
        let h = v.password_history.as_deref().unwrap_or_default();
        return some(h.get(usize::from(back) - 1).map(|h| &h.password));
    }
    match key {
        BwKey::Password => some(login()?.password.as_ref()),
        BwKey::Username => some(login()?.username.as_ref()),
        BwKey::Notes => some(v.notes.as_ref()),
        BwKey::Uri => some(login()?.uris.iter().flatten().find_map(|u| u.uri.as_ref())),
        BwKey::TotpSeed => {
            if !cfg.expose_totp_seed {
                return Err(ExtError::Denied(format!(
                    "bw/{conn} does not expose TOTP seeds, only the current code (#totp); \
                     Settings › Secrets › {conn}"
                )));
            }
            some(login()?.totp.as_ref())
        }
        BwKey::Totp => {
            let seed = login()?.totp.clone().ok_or(ExtError::NotFound)?;
            let code = bitwarden_vault::generate_totp(seed, now)
                .map_err(|e| ExtError::Sdk(format!("TOTP: {e}")))?;
            Ok(Zeroizing::new(code.code))
        }
        BwKey::Field(name) => {
            let f = v
                .fields
                .iter()
                .flatten()
                .find(|f| f.name.as_deref() == Some(name.as_str()))
                .ok_or(ExtError::NotFound)?;
            match f.r#type {
                FieldType::Text | FieldType::Hidden => some(f.value.as_ref()),
                FieldType::Boolean => Ok(Zeroizing::new(
                    (f.value.as_deref() == Some("true")).to_string(),
                )),
                FieldType::Linked => match f.linked_id.map(u32::from) {
                    Some(LINKED_USERNAME) => some(login()?.username.as_ref()),
                    Some(LINKED_PASSWORD) => some(login()?.password.as_ref()),
                    _ => Err(ExtError::Unsupported(
                        "this linked field points at a card or identity field".into(),
                    )),
                },
            }
        }
    }
}

/// Write `value` into the field `key` names on an edit request (221 §5.6). A custom field must
/// already exist; `#totp` and `#totp-seed` both set the seed.
pub fn write_field(
    req: &mut CipherEditRequest,
    key: Option<&BwKey>,
    value: &str,
) -> Result<(), ExtError> {
    let is_login = matches!(req.r#type, CipherViewType::Login(_));
    if !is_login && !matches!(req.r#type, CipherViewType::SecureNote(_)) {
        return Err(ExtError::Unsupported(
            "only logins and secure notes can be written".into(),
        ));
    }
    let key = key.cloned().unwrap_or(if is_login {
        BwKey::Password
    } else {
        BwKey::Notes
    });
    let v = Some(value.to_owned());
    match key {
        BwKey::Notes => req.notes = v,
        BwKey::Field(name) => {
            let f = req
                .fields
                .iter_mut()
                .find(|f| f.name.as_deref() == Some(name.as_str()))
                .ok_or(ExtError::NotFound)?;
            if f.r#type == FieldType::Linked {
                return Err(ExtError::Unsupported(
                    "a linked field has no value of its own; write the field it points at".into(),
                ));
            }
            f.value = v;
        }
        k => {
            let CipherViewType::Login(l) = &mut req.r#type else {
                return Err(ExtError::Unsupported(format!(
                    "#{} exists on logins only",
                    k.as_key()
                )));
            };
            match k {
                BwKey::Password => l.password = v,
                BwKey::Username => l.username = v,
                BwKey::Totp | BwKey::TotpSeed => l.totp = v,
                BwKey::Uri => match l.uris.as_mut().and_then(|u| u.first_mut()) {
                    Some(u) => {
                        u.uri = v;
                        u.uri_checksum = None;
                    }
                    None => l.uris = Some(vec![uri(value)]),
                },
                BwKey::Notes | BwKey::Field(_) => unreachable!("handled above"),
            }
        }
    }
    Ok(())
}

fn uri(u: &str) -> LoginUriView {
    LoginUriView {
        uri: Some(u.to_owned()),
        r#match: None,
        uri_checksum: None,
    }
}

/// The reference key a created item's value lives under.
pub fn created_key(field: &BwField) -> BwKey {
    match field {
        BwField::Password => BwKey::Password,
        BwField::Notes => BwKey::Notes,
        BwField::Totp => BwKey::TotpSeed,
        BwField::Custom { name, .. } => BwKey::Field(name.clone()),
    }
}

/// Everything of `Location::Bw` the create request needs besides the ids.
pub struct NewItem<'a> {
    /// Login or secure note.
    pub kind: BwKind,
    /// The item's name.
    pub item_name: &'a str,
    /// Which field `value` fills.
    pub field: &'a BwField,
    /// Login username.
    pub username: Option<&'a str>,
    /// Login URI.
    pub uri: Option<&'a str>,
}

/// A create request for `item` holding `value`; ids (org, collections, folder) are the caller's.
pub fn create_request(item: &NewItem<'_>, value: &str) -> Result<CipherCreateRequest, ExtError> {
    let v = Some(value.to_owned());
    let mut notes = None;
    let mut fields = Vec::new();
    let mut login = LoginView {
        username: item.username.map(str::to_owned),
        password: None,
        password_revision_date: None,
        uris: item.uri.map(|u| vec![uri(u)]),
        totp: None,
        autofill_on_page_load: None,
        http_authentication_enabled: None,
        fido2_credentials: None,
    };
    match (item.kind, item.field) {
        (_, BwField::Notes) => notes = v,
        (_, BwField::Custom { name, hidden }) => fields.push(FieldView {
            name: Some(name.clone()),
            value: v,
            r#type: if *hidden {
                FieldType::Hidden
            } else {
                FieldType::Text
            },
            linked_id: None,
        }),
        (BwKind::Login, BwField::Password) => login.password = v,
        (BwKind::Login, BwField::Totp) => login.totp = v,
        (BwKind::SecureNote, f) => {
            return Err(ExtError::Unsupported(format!(
                "a secure note has no {} field; use notes or a custom field",
                match f {
                    BwField::Password => "password",
                    _ => "TOTP",
                }
            )));
        }
    }
    let r#type = match item.kind {
        BwKind::Login => CipherViewType::Login(login),
        BwKind::SecureNote => CipherViewType::SecureNote(SecureNoteView {
            r#type: SecureNoteType::Generic,
        }),
    };
    Ok(CipherCreateRequest {
        organization_id: None,
        collection_ids: Vec::new(),
        folder_id: None,
        name: item.item_name.to_owned(),
        notes,
        favorite: false,
        reprompt: CipherRepromptType::None,
        r#type,
        fields,
        archived_date: None,
    })
}

#[cfg(test)]
mod tests {
    use bitwarden_vault::{CipherId, PasswordHistoryView};

    use super::*;

    fn login_view() -> CipherView {
        let t = DateTime::parse_from_rfc3339("2026-10-30T12:00:00.123Z")
            .unwrap()
            .to_utc();
        CipherView {
            id: Some(CipherId::new(uuid::Uuid::nil())),
            organization_id: None,
            folder_id: None,
            collection_ids: vec![],
            key: None,
            name: "Prod DB".into(),
            notes: Some("note text".into()),
            r#type: CipherType::Login,
            login: Some(LoginView {
                username: Some("admin".into()),
                password: Some("pw-now".into()),
                password_revision_date: None,
                uris: Some(vec![uri("https://db.example")]),
                totp: Some("JBSWY3DPEHPK3PXP".into()),
                autofill_on_page_load: None,
                http_authentication_enabled: None,
                fido2_credentials: None,
            }),
            identity: None,
            card: None,
            secure_note: None,
            ssh_key: None,
            bank_account: None,
            drivers_license: None,
            passport: None,
            favorite: false,
            reprompt: CipherRepromptType::None,
            organization_use_totp: false,
            edit: true,
            permissions: None,
            view_password: true,
            local_data: None,
            attachments: None,
            attachment_decryption_failures: None,
            fields: Some(vec![
                FieldView {
                    name: Some("api".into()),
                    value: Some("tok".into()),
                    r#type: FieldType::Hidden,
                    linked_id: None,
                },
                FieldView {
                    name: Some("flag".into()),
                    value: Some("true".into()),
                    r#type: FieldType::Boolean,
                    linked_id: None,
                },
                FieldView {
                    name: Some("link".into()),
                    value: None,
                    r#type: FieldType::Linked,
                    linked_id: Some(LINKED_USERNAME.try_into().unwrap()),
                },
                FieldView {
                    name: Some("api".into()),
                    value: Some("second".into()),
                    r#type: FieldType::Text,
                    linked_id: None,
                },
            ]),
            password_history: Some(vec![
                PasswordHistoryView {
                    password: "pw-1".into(),
                    last_used_date: t,
                },
                PasswordHistoryView {
                    password: "pw-2".into(),
                    last_used_date: t,
                },
            ]),
            creation_date: t,
            deleted_date: None,
            revision_date: t,
            archived_date: None,
            partial: false,
        }
    }

    fn read(
        v: &CipherView,
        key: Option<&str>,
        back: u8,
        cfg: &BwConfig,
    ) -> Result<String, ExtError> {
        let key = key.map(|k| BwKey::parse(k).unwrap());
        read_field(v, key.as_ref(), back, cfg, "personal", None).map(|z| z.to_string())
    }

    #[test]
    fn reads_login_fields() {
        let v = login_view();
        let cfg = BwConfig::default();
        assert_eq!(read(&v, None, 0, &cfg).unwrap(), "pw-now");
        assert_eq!(read(&v, Some("password"), 0, &cfg).unwrap(), "pw-now");
        assert_eq!(read(&v, Some("username"), 0, &cfg).unwrap(), "admin");
        assert_eq!(read(&v, Some("notes"), 0, &cfg).unwrap(), "note text");
        assert_eq!(
            read(&v, Some("uri"), 0, &cfg).unwrap(),
            "https://db.example"
        );
        // The first custom field with that exact name.
        assert_eq!(read(&v, Some("field:api"), 0, &cfg).unwrap(), "tok");
        assert_eq!(read(&v, Some("field:flag"), 0, &cfg).unwrap(), "true");
        assert_eq!(read(&v, Some("field:link"), 0, &cfg).unwrap(), "admin");
        assert_eq!(
            read(&v, Some("field:nope"), 0, &cfg),
            Err(ExtError::NotFound)
        );
        // TOTP: the current code, never the seed.
        let code = read(&v, Some("totp"), 0, &cfg).unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|b| b.is_ascii_digit()));
        assert_ne!(code, "JBSWY3DPEHPK3PXP");
    }

    #[test]
    fn seed_and_history_follow_their_settings() {
        let v = login_view();
        let off = BwConfig::default();
        assert!(matches!(
            read(&v, Some("totp-seed"), 0, &off),
            Err(ExtError::Denied(_))
        ));
        assert!(matches!(
            read(&v, Some("password"), 1, &off),
            Err(ExtError::Denied(_))
        ));
        let on = BwConfig {
            expose_totp_seed: true,
            expose_password_history: true,
            ..off
        };
        assert_eq!(
            read(&v, Some("totp-seed"), 0, &on).unwrap(),
            "JBSWY3DPEHPK3PXP"
        );
        assert_eq!(read(&v, None, 1, &on).unwrap(), "pw-1");
        assert_eq!(read(&v, Some("password"), 2, &on).unwrap(), "pw-2");
        assert_eq!(read(&v, Some("password"), 3, &on), Err(ExtError::NotFound));
        assert!(matches!(
            read(&v, Some("username"), 1, &on),
            Err(ExtError::Unsupported(_))
        ));
    }

    #[test]
    fn secure_notes_default_to_notes_and_cards_are_unsupported() {
        let mut v = login_view();
        v.r#type = CipherType::SecureNote;
        v.login = None;
        let cfg = BwConfig::default();
        assert_eq!(read(&v, None, 0, &cfg).unwrap(), "note text");
        assert_eq!(read(&v, Some("password"), 0, &cfg), Err(ExtError::NotFound));
        v.r#type = CipherType::Card;
        assert!(matches!(
            read(&v, None, 0, &cfg),
            Err(ExtError::Unsupported(_))
        ));
    }

    #[test]
    fn keys_list_names_only_once() {
        assert_eq!(
            field_keys(&login_view()),
            [
                "password",
                "username",
                "totp",
                "uri",
                "notes",
                "field:api",
                "field:flag",
                "field:link"
            ]
        );
    }

    #[test]
    fn writes_replace_the_named_field() {
        let mut req = CipherEditRequest::try_from(login_view()).unwrap();
        write_field(&mut req, None, "new-pw").unwrap();
        write_field(&mut req, Some(&BwKey::Field("api".into())), "new-tok").unwrap();
        write_field(&mut req, Some(&BwKey::Uri), "https://new.example").unwrap();
        assert_eq!(
            write_field(&mut req, Some(&BwKey::Field("missing".into())), "x"),
            Err(ExtError::NotFound)
        );
        assert!(write_field(&mut req, Some(&BwKey::Field("link".into())), "x").is_err());
        let CipherViewType::Login(l) = &req.r#type else {
            panic!("login")
        };
        assert_eq!(l.password.as_deref(), Some("new-pw"));
        assert_eq!(
            l.uris.as_ref().unwrap()[0].uri.as_deref(),
            Some("https://new.example")
        );
        assert_eq!(req.fields[0].value.as_deref(), Some("new-tok"));
        assert_eq!(req.fields[3].value.as_deref(), Some("second"));
    }

    #[test]
    fn create_requests_by_kind_and_field() {
        let custom = BwField::Custom {
            name: "token".into(),
            hidden: true,
        };
        let req = create_request(
            &NewItem {
                kind: BwKind::Login,
                item_name: "CI",
                field: &BwField::Password,
                username: Some("bot"),
                uri: Some("https://ci.example"),
            },
            "s3cret",
        )
        .unwrap();
        let CipherViewType::Login(l) = &req.r#type else {
            panic!("login")
        };
        assert_eq!(l.password.as_deref(), Some("s3cret"));
        assert_eq!(l.username.as_deref(), Some("bot"));
        assert_eq!(req.name, "CI");

        let req = create_request(
            &NewItem {
                kind: BwKind::SecureNote,
                item_name: "n",
                field: &custom,
                username: None,
                uri: None,
            },
            "v",
        )
        .unwrap();
        assert!(matches!(req.r#type, CipherViewType::SecureNote(_)));
        assert_eq!(req.fields[0].r#type, FieldType::Hidden);
        assert_eq!(req.fields[0].value.as_deref(), Some("v"));
        assert_eq!(created_key(&custom), BwKey::Field("token".into()));

        assert!(
            create_request(
                &NewItem {
                    kind: BwKind::SecureNote,
                    item_name: "n",
                    field: &BwField::Password,
                    username: None,
                    uri: None
                },
                "v"
            )
            .is_err()
        );
    }

    #[test]
    fn versions_compare_as_instants() {
        let t = login_view().revision_date;
        let v = version(t);
        assert_eq!(v.id, "2026-10-30T12:00:00.123Z");
        assert!(same_version(&v, t));
        let other = ExtVersion {
            id: "2026-10-30T13:00:00.123+01:00".into(),
            ..v.clone()
        };
        assert!(same_version(&other, t));
        assert!(!same_version(
            &ExtVersion {
                id: "garbage".into(),
                ..v
            },
            t
        ));
    }
}
