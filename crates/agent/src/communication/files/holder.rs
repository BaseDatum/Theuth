//! 199 holder RPCs (`(holder, id)`; the RPC layer carries a call for another engine to
//! it): the card, a bounded thumbnail, and a no-clobber export. Bytes never leave as
//! base64 except the fitted thumbnail.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;

use super::{ContactFile, ContactFileThumbnail};
use crate::store::Store;
use crate::store::contact_files::{FileRow, Render};

/// Read at most this much for a thumbnail.
pub const THUMB_READ_MAX: u64 = 64 * 1024 * 1024;
/// Longest edge of a thumbnail.
const THUMB_EDGE: u32 = 512;

/// This engine as cards name it: its endpoint id and machine name.
pub async fn render(store: &Store) -> Render {
    Render {
        holder: store.engine().to_owned(),
        machine: crate::communication::mesh::engine_names().await.0,
    }
}

fn no_such(r: &Render) -> String {
    format!("no such file on {}", r.machine)
}

async fn row(store: &Store, r: &Render, id: &str) -> Result<FileRow, String> {
    store
        .contact_file(id)
        .await
        .map_err(|e| format!("{e:#}"))?
        .filter(|row| !row.exchange.starts_with("pending:"))
        .ok_or_else(|| no_such(r))
}

/// The bytes a card of `row` may show: the saved file (in), the clone or the original
/// (out). Never past the declared size.
fn local(row: &FileRow) -> Option<PathBuf> {
    match row.direction.as_str() {
        "in" => row
            .path
            .clone()
            .filter(|_| matches!(row.state.as_str(), "saved" | "received"))
            .map(Into::into),
        _ => row
            .clone
            .clone()
            .filter(|c| Path::new(c).exists())
            .or_else(|| Some(row.source.clone()))
            .map(Into::into),
    }
}

/// `contact_file(holder, id)` on the holder.
pub async fn card(store: &Store, r: &Render, id: &str) -> Result<ContactFile, String> {
    let row = row(store, r, id).await?;
    let alias = store
        .contact_alias(&row.contact_id)
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(row.card(r, &alias))
}

/// `contact_file_thumbnail(holder, id)` on the holder: at most [`THUMB_READ_MAX`] read,
/// the guarded decoder fits it to 512 px; its actual MIME. Errors (`not an image`, `too
/// large for a thumbnail`) tell the card to show a type icon.
pub async fn thumbnail(
    store: &Store,
    r: &Render,
    id: &str,
) -> Result<ContactFileThumbnail, String> {
    let row = row(store, r, id).await?;
    let path = local(&row).ok_or_else(|| no_such(r))?;
    let declared = row.size.unwrap_or(i64::MAX) as u64;
    let missing = no_such(r);
    tokio::task::spawn_blocking(move || {
        let f = std::fs::File::open(&path).map_err(|_| missing)?;
        let len = f.metadata().map_err(|e| e.to_string())?.len().min(declared);
        if len > THUMB_READ_MAX {
            return Err("too large for a thumbnail".to_owned());
        }
        let mut bytes = Vec::with_capacity(len as usize);
        f.take(len)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let fitted =
            tools::images::fit(&bytes, THUMB_EDGE).map_err(|_| "not an image".to_owned())?;
        Ok(ContactFileThumbnail {
            mime: fitted.mime.to_owned(),
            data: base64::engine::general_purpose::STANDARD.encode(&fitted.bytes),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `contact_file_export(holder, id, dest)` on the holder: a copy of the saved file at
/// `dest`, which must not exist (never replaced).
pub async fn export(store: &Store, r: &Render, id: &str, dest: PathBuf) -> Result<PathBuf, String> {
    let row = row(store, r, id).await?;
    let src = local(&row)
        .filter(|_| row.direction == "in")
        .ok_or_else(|| no_such(r))?;
    if !dest.is_absolute() {
        return Err(format!("{} is not an absolute path", dest.display()));
    }
    let missing = no_such(r);
    tokio::task::spawn_blocking(move || {
        let mut from = std::fs::File::open(&src).map_err(|_| missing)?;
        let mut to = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(format!("{} exists", dest.display()));
            }
            Err(e) => return Err(format!("{}: {e}", dest.display())),
        };
        if let Err(e) = std::io::copy(&mut from, &mut to).and_then(|_| to.sync_all()) {
            drop(to);
            let _ = std::fs::remove_file(&dest);
            return Err(format!("{}: {e}", dest.display()));
        }
        Ok(dest)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (Store, crate::TestDir, Render) {
        let d = crate::TestDir::new("holder");
        let store = Store::open(&d.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let r = Render {
            holder: "h".into(),
            machine: "Mac".into(),
        };
        (store, d, r)
    }

    fn saved(id: &str, path: &Path, size: usize) -> FileRow {
        FileRow {
            id: id.into(),
            direction: "in".into(),
            session: "s".into(),
            exchange: "e".into(),
            contact_id: "rec".into(),
            name: "x".into(),
            media_type: "image/png".into(),
            size: Some(size as i64),
            source: "iroh://h/a2a/files/0".into(),
            path: Some(path.to_string_lossy().into_owned()),
            state: "received".into(),
            ..Default::default()
        }
    }

    fn png() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(1024, 600, image::Rgb([200, 10, 10]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// A picture is fitted to 512 px with its actual MIME; a non-image and an oversize
    /// file are refused (the card falls back to an icon); an unknown id names the machine.
    #[tokio::test]
    async fn thumbnails_are_bounded_and_fall_back() {
        let (store, d, r) = fixture().await;
        let pic = d.0.join("p.png");
        std::fs::write(&pic, png()).unwrap();
        let text = d.0.join("t.txt");
        std::fs::write(&text, b"hello").unwrap();
        let big = d.0.join("big.png");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(THUMB_READ_MAX + 1)
            .unwrap();
        let rows = [
            saved(&"1".repeat(32), &pic, png().len()),
            saved(&"2".repeat(32), &text, 5),
            saved(&"3".repeat(32), &big, THUMB_READ_MAX as usize + 1),
        ];
        store.insert_contact_files(&rows).await.unwrap();
        let t = thumbnail(&store, &r, &rows[0].id).await.unwrap();
        assert_eq!(t.mime, "image/png");
        let raw = base64::engine::general_purpose::STANDARD
            .decode(t.data)
            .unwrap();
        let img = image::load_from_memory(&raw).unwrap();
        assert_eq!((img.width(), img.height()), (512, 300));
        assert_eq!(
            thumbnail(&store, &r, &rows[1].id).await.unwrap_err(),
            "not an image"
        );
        assert_eq!(
            thumbnail(&store, &r, &rows[2].id).await.unwrap_err(),
            "too large for a thumbnail"
        );
        assert_eq!(
            thumbnail(&store, &r, &"9".repeat(32)).await.unwrap_err(),
            "no such file on Mac"
        );
    }

    /// Export copies the saved bytes and never replaces an existing file.
    #[tokio::test]
    async fn export_never_clobbers() {
        let (store, d, r) = fixture().await;
        let src = d.0.join("report.pdf");
        std::fs::write(&src, b"ours").unwrap();
        let row = saved(&"4".repeat(32), &src, 4);
        store
            .insert_contact_files(std::slice::from_ref(&row))
            .await
            .unwrap();
        let dest = d.0.join("out.pdf");
        assert_eq!(
            export(&store, &r, &row.id, dest.clone()).await.unwrap(),
            dest
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"ours");
        std::fs::write(&dest, b"theirs").unwrap();
        let e = export(&store, &r, &row.id, dest.clone()).await.unwrap_err();
        assert_eq!(e, format!("{} exists", dest.display()));
        assert_eq!(std::fs::read(&dest).unwrap(), b"theirs");
    }
}
