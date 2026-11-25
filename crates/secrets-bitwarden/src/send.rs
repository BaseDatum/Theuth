//! Bitwarden Sends (221 §12, milestones B5c/B5d): text and file Sends through the SDK's
//! `SendClient`, the share link (G8), and Bitwarden attachments as a file source. The content
//! arrives here already resolved; it never reaches the model, the args or an event.

use bitwarden_core::ApiError;
use bitwarden_send::{
    CreateFileSendError, CreateSendError, DeleteSendError, SendAddRequest, SendAuthType,
    SendClientExt as _, SendFileView, SendId, SendTextView, SendView, SendViewType,
    UploadSendFileError,
};
use bitwarden_vault::{Cipher, CipherId, VaultClientExt as _};
use chrono::{DateTime, Utc};
use secrets::external::{ExtError, ExtRef};
use secrets::tools_send::{CreatedSend, SendBackend, SendContent, SendInfo, SendOptions};
use zeroize::Zeroizing;

use crate::{BwBackend, api_err, cipher_id, sdk, server_message};

/// The longest a Send may live (the servers refuse more).
pub const MAX_DELETION_DAYS: i64 = 31;

/// G8: the share link. `key_b64` is the Send's standard-base64 key; the fragment carries it
/// URL-safe and unpadded, as the web vault and `bw receive` read it.
pub fn send_link(web_vault: &str, access_id: &str, key_b64: &str) -> String {
    let key: String = key_b64
        .chars()
        .filter(|c| *c != '=')
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    format!(
        "{}/#/send/{access_id}/{key}",
        web_vault.trim_end_matches('/')
    )
}

/// A server refusal of a Send create, in the words §12.3 gives the model.
fn send_refusal(status: u16, body: &str, file: bool) -> ExtError {
    let msg = server_message(body);
    let lower = msg.to_lowercase();
    if file && lower.contains("premium") {
        return ExtError::Denied(
            "file Sends need Premium on bitwarden.com; text Sends work".into(),
        );
    }
    if lower.contains("disabled") || lower.contains("not allowed") || lower.contains("policy") {
        return ExtError::Denied(
            "Sends are disabled on this server (SENDS_ALLOWED) or by your organisation's policy"
                .into(),
        );
    }
    if lower.contains("storage") || lower.contains("limit") || lower.contains("too large") {
        return ExtError::Denied(format!("the server refused the file: {msg}"));
    }
    if lower.contains("31 days") || lower.contains("deletion date") {
        return ExtError::Denied("delete must be within 31 days".into());
    }
    ExtError::Sdk(format!("Bitwarden answered {status}: {msg}"))
}

fn api_refusal(e: &ApiError, file: bool) -> ExtError {
    match e {
        ApiError::Response(r) if r.status.as_u16() == 400 => {
            send_refusal(r.status.as_u16(), &r.message, file)
        }
        e => api_err(e),
    }
}

/// A failed file upload, without the upload URL (an Azure SAS token rides in it): a server
/// refusal keeps the server's words; transport failures are named by kind only, in the log too.
fn upload_error(e: &UploadSendFileError) -> ExtError {
    let kind = match e {
        UploadSendFileError::Api(e @ ApiError::Response(_)) => return api_refusal(e, true),
        UploadSendFileError::Api(_) => "network",
        UploadSendFileError::Reqwest(_) => "multipart form",
        UploadSendFileError::RenewFileUploadUrl(_) => "upload URL renewal",
    };
    tracing::warn!(kind, "bitwarden: Send upload failed");
    ExtError::Network(format!("upload to Bitwarden failed ({kind})"))
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn request(opts: &SendOptions, view_type: SendViewType) -> SendAddRequest {
    SendAddRequest {
        name: opts.name.clone(),
        notes: opts.notes.clone(),
        view_type,
        max_access_count: opts.max_access_count,
        disabled: false,
        hide_email: opts.hide_email,
        deletion_date: opts.deletion,
        expiration_date: opts.expiration,
        auth: match &opts.password {
            Some(p) => SendAuthType::Password {
                password: p.to_string(),
            },
            None => SendAuthType::None,
        },
    }
}

impl BwBackend {
    /// Create a text or file Send (§12.2). A file Send whose upload fails is deleted again, so
    /// no fileless Send stays behind.
    pub async fn create_send(
        &self,
        content: SendContent,
        opts: SendOptions,
    ) -> Result<CreatedSend, ExtError> {
        if opts.deletion > Utc::now() + chrono::Duration::days(MAX_DELETION_DAYS) {
            return Err(ExtError::Denied("delete must be within 31 days".into()));
        }
        if opts.expiration.is_some_and(|e| e > opts.deletion) {
            return Err(ExtError::Denied(
                "the link must expire before the Send is deleted".into(),
            ));
        }
        let web = self
            .web_vault_url()
            .ok_or_else(|| ExtError::Unsupported("no web vault URL".into()))?
            .to_owned();
        let mut guard = self.ready().await?;
        self.touch();
        let s = guard.as_mut().expect("ready() checked");
        self.fresh(s, true).await?;
        let sends = s.client.sends();
        let view = match content {
            SendContent::Text(text) => {
                let req = request(
                    &opts,
                    SendViewType::Text(SendTextView {
                        text: Some(text.to_string()),
                        hidden: true,
                    }),
                );
                drop(text);
                let v = self.timed(sends.create(req)).await?.map_err(|e| match e {
                    CreateSendError::Api(e) => api_refusal(&e, false),
                    e => sdk(e),
                })?;
                v
            }
            SendContent::File { name, bytes } => {
                let req = request(
                    &opts,
                    SendViewType::File(SendFileView {
                        id: None,
                        file_name: name.clone(),
                        size: None,
                        size_name: None,
                    }),
                );
                let created = self
                    .timed(sends.create_file_send(req, bytes.to_vec()))
                    .await?
                    .map_err(|e| match e {
                        CreateFileSendError::Api(e) => api_refusal(&e, true),
                        e => sdk(e),
                    })?;
                drop(bytes);
                let send_id = created
                    .send
                    .id
                    .ok_or_else(|| ExtError::Sdk("created Send has no id".into()))?;
                let upload = self
                    .timed(sends.upload_send_file(
                        send_id,
                        created.file_id.clone(),
                        created.encrypted_file_name.clone(),
                        created.file_upload_type,
                        created.url.clone(),
                        created.encrypted_file_buffer,
                    ))
                    .await;
                let failed = match upload {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(upload_error(&e)),
                    Err(e) => Some(e),
                };
                if let Some(e) = failed {
                    let gone = self
                        .timed(sends.delete(send_id))
                        .await
                        .is_ok_and(|r| r.is_ok());
                    // No sweeper retries a Send theuth holds no row for: name it so the user can.
                    let tail = if gone {
                        "; the Send was deleted".to_owned()
                    } else {
                        format!(
                            "; deleting the Send failed: Send {send_id} has no file and still \
                             exists, delete it in Bitwarden (Send tab)"
                        )
                    };
                    return Err(match e {
                        ExtError::Network(m) => ExtError::Network(format!("{m}{tail}")),
                        ExtError::Denied(m) => ExtError::Denied(format!("{m}{tail}")),
                        ExtError::Sdk(m) => ExtError::Sdk(format!("{m}{tail}")),
                        e => ExtError::Sdk(format!("{e}{tail}")),
                    });
                }
                created.send
            }
        };
        created_send(&web, view)
    }

    /// Delete a Send (revoke). An unknown id is `NotFound`.
    pub async fn delete_send(&self, send_id: &str) -> Result<(), ExtError> {
        let id: SendId = send_id.parse().map_err(|_| ExtError::NotFound)?;
        let mut guard = self.ready().await?;
        let s = guard.as_mut().expect("ready() checked");
        self.timed(s.client.sends().delete(id))
            .await?
            .map_err(|e| match e {
                DeleteSendError::Api(e) => api_err(&e),
                e => sdk(e),
            })
    }

    /// The account's Sends after a sync (access counts are current).
    pub async fn list_sends(&self) -> Result<Vec<SendInfo>, ExtError> {
        let mut guard = self.ready().await?;
        let s = guard.as_mut().expect("ready() checked");
        self.sync(s, false).await?;
        let list = s.client.sends().list().await.map_err(sdk)?;
        Ok(list
            .into_iter()
            .filter_map(|v| {
                Some(SendInfo {
                    send_id: v.id?.to_string(),
                    kind: if v.file.is_some() { "file" } else { "text" }.into(),
                    file_name: v.file.as_ref().map(|f| f.file_name.clone()),
                    name: v.name,
                    access_count: v.access_count,
                    max_access_count: v.max_access_count,
                    has_password: v.has_password,
                    deletion_date: rfc3339(v.deletion_date),
                    expiration_date: v.expiration_date.map(rfc3339),
                    disabled: v.disabled,
                })
            })
            .collect())
    }

    /// Attachment `which` (id or file name) of item `r`: its file name and recorded size.
    pub async fn attachment_info(
        &self,
        r: &ExtRef,
        which: &str,
    ) -> Result<(String, Option<u64>), ExtError> {
        let id = cipher_id(r)?;
        let mut guard = self.ready().await?;
        let s = guard.as_mut().expect("ready() checked");
        self.fresh(s, false).await?;
        let v = self.live(s, id).await?;
        let a = v
            .attachments
            .unwrap_or_default()
            .into_iter()
            .find(|a| a.id.as_deref() == Some(which) || a.file_name.as_deref() == Some(which))
            .ok_or(ExtError::NotFound)?;
        Ok((
            a.file_name.or(a.id).unwrap_or_default(),
            a.size.and_then(|s| s.parse().ok()),
        ))
    }

    /// Download and decrypt attachment `which` (id or file name) of item `r`, refusing before the
    /// download when its recorded size exceeds `max_bytes`.
    pub async fn attachment_bytes(
        &self,
        r: &ExtRef,
        which: &str,
        max_bytes: u64,
    ) -> Result<(String, Zeroizing<Vec<u8>>), ExtError> {
        let id: CipherId = cipher_id(r)?;
        let mut guard = self.ready().await?;
        self.touch();
        let s = guard.as_mut().expect("ready() checked");
        self.fresh(s, false).await?;
        let v = self.live(s, id).await?;
        let att = v
            .attachments
            .clone()
            .unwrap_or_default()
            .into_iter()
            .find(|a| a.id.as_deref() == Some(which) || a.file_name.as_deref() == Some(which))
            .ok_or(ExtError::NotFound)?;
        let size: Option<u64> = att.size.as_deref().and_then(|s| s.parse().ok());
        if size.is_some_and(|n| n > max_bytes) {
            return Err(ExtError::Denied(format!(
                "the attachment is {} bytes; the limit here is {max_bytes}",
                size.unwrap_or_default()
            )));
        }
        let att_id = att.id.clone().ok_or(ExtError::NotFound)?;
        let name = att.file_name.clone().unwrap_or_else(|| att_id.clone());
        let attachments = s.client.vault().attachments();
        let url = self
            .timed(attachments.get_attachment_download_url(id, att_id, None))
            .await?
            .map_err(sdk)?;
        let api = s.client.internal.get_api_configurations();
        // The download URL is signed: no error text that could carry it.
        let resp = self
            .timed(api.identity_config.client.get(url).send())
            .await?
            .map_err(|_| ExtError::Network("attachment download failed".into()))?;
        if !resp.status().is_success() {
            return Err(ExtError::Network(format!(
                "attachment download answered {}",
                resp.status().as_u16()
            )));
        }
        let enc = self
            .timed(resp.bytes())
            .await?
            .map_err(|_| ExtError::Network("attachment download broke off".into()))?;
        if enc.len() as u64 > max_bytes.saturating_add(1024) {
            return Err(ExtError::Denied(format!(
                "the attachment exceeds {max_bytes} bytes"
            )));
        }
        let cipher: Cipher = s
            .client
            .platform()
            .state()
            .get::<Cipher>()
            .map_err(sdk)?
            .get(id)
            .await
            .map_err(sdk)?
            .ok_or(ExtError::NotFound)?;
        let plain = attachments.decrypt_buffer(cipher, att, &enc).map_err(sdk)?;
        Ok((name, Zeroizing::new(plain)))
    }
}

fn created_send(web: &str, v: SendView) -> Result<CreatedSend, ExtError> {
    let send_id =
        v.id.ok_or_else(|| ExtError::Sdk("created Send has no id".into()))?;
    let access_id = v
        .access_id
        .ok_or_else(|| ExtError::Sdk("created Send has no access id".into()))?;
    let key = Zeroizing::new(
        v.key
            .ok_or_else(|| ExtError::Sdk("created Send has no key".into()))?,
    );
    Ok(CreatedSend {
        link: Zeroizing::new(send_link(web, &access_id, &key)),
        send_id: send_id.to_string(),
        access_id,
        deletion_date: rfc3339(v.deletion_date),
        expiration_date: v.expiration_date.map(rfc3339),
    })
}

#[async_trait::async_trait]
impl SendBackend for BwBackend {
    async fn create_send(
        &self,
        content: SendContent,
        opts: SendOptions,
    ) -> Result<CreatedSend, ExtError> {
        BwBackend::create_send(self, content, opts).await
    }

    async fn delete_send(&self, send_id: &str) -> Result<(), ExtError> {
        BwBackend::delete_send(self, send_id).await
    }

    async fn list_sends(&self) -> Result<Vec<SendInfo>, ExtError> {
        BwBackend::list_sends(self).await
    }

    async fn attachment_info(
        &self,
        r: &ExtRef,
        which: &str,
    ) -> Result<(String, Option<u64>), ExtError> {
        BwBackend::attachment_info(self, r, which).await
    }

    async fn attachment_bytes(
        &self,
        r: &ExtRef,
        which: &str,
        max_bytes: u64,
    ) -> Result<(String, Zeroizing<Vec<u8>>), ExtError> {
        BwBackend::attachment_bytes(self, r, which, max_bytes).await
    }

    fn send_owner_only(&self) -> bool {
        self.cfg
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send_owner_only
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_format_for_each_server() {
        let key = "7dw+xzkQ/tdfpmz24rgmHQ==";
        for (web, want) in [
            (
                "https://vault.bitwarden.com",
                "https://vault.bitwarden.com/#/send/A68iPF2E/7dw-xzkQ_tdfpmz24rgmHQ",
            ),
            (
                "https://vault.bitwarden.eu",
                "https://vault.bitwarden.eu/#/send/A68iPF2E/7dw-xzkQ_tdfpmz24rgmHQ",
            ),
            (
                "https://vault.example.org/",
                "https://vault.example.org/#/send/A68iPF2E/7dw-xzkQ_tdfpmz24rgmHQ",
            ),
        ] {
            assert_eq!(send_link(web, "A68iPF2E", key), want);
        }
    }

    #[test]
    fn refusals_map_to_model_text() {
        let premium = send_refusal(
            400,
            r#"{"message":"You must have premium status to use file Sends.","object":"error"}"#,
            true,
        );
        assert_eq!(
            premium,
            ExtError::Denied("file Sends need Premium on bitwarden.com; text Sends work".into())
        );
        let disabled = send_refusal(400, r#"{"message":"Sends are disabled."}"#, false);
        assert!(matches!(&disabled, ExtError::Denied(m) if m.contains("SENDS_ALLOWED")));
        let quota = send_refusal(
            400,
            r#"{"message":"Send storage limit exceeded with this file"}"#,
            true,
        );
        assert!(matches!(&quota, ExtError::Denied(m) if m.contains("storage limit")));
        let far = send_refusal(
            400,
            r#"{"message":"You cannot have a Send with a deletion date that far into the future. Adjust the Deletion Date to a value less than 31 days from now and try again."}"#,
            false,
        );
        assert_eq!(
            far,
            ExtError::Denied("delete must be within 31 days".into())
        );
    }
}
