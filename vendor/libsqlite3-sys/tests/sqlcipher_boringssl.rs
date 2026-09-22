//! theuth: SQLCipher compiled against BoringSSL must read and write the stock
//! SQLCipher 4 format. `openssl-provider.db` was written by the OpenSSL-provider
//! `sqlcipher` CLI (4.19.0, Homebrew) with `PRAGMA key='k'; CREATE TABLE t(a);
//! INSERT INTO t VALUES(1);`. A broken HMAC shim fails `opens_openssl_provider_db`
//! (every page's HMAC mismatches → "file is not a database").
#![cfg(feature = "bundled-sqlcipher")]

use core::ffi::{c_char, c_int, c_void, CStr};
use std::ffi::CString;
use std::path::Path;
use std::ptr;

use libsqlite3_sys as ffi;

struct Db(*mut ffi::sqlite3);

impl Db {
    fn open(path: &Path, flags: c_int) -> Db {
        let path = CString::new(path.to_str().unwrap()).unwrap();
        let mut db = ptr::null_mut();
        let rc = unsafe { ffi::sqlite3_open_v2(path.as_ptr(), &mut db, flags, ptr::null()) };
        assert_eq!(rc, ffi::SQLITE_OK, "sqlite3_open_v2 failed: {rc}");
        Db(db)
    }

    /// Runs `sql`; returns the first column of every row, or `(rc, message)`.
    fn query(&self, sql: &str) -> Result<Vec<String>, (c_int, String)> {
        unsafe extern "C" fn collect(
            rows: *mut c_void,
            ncols: c_int,
            values: *mut *mut c_char,
            _names: *mut *mut c_char,
        ) -> c_int {
            let rows = unsafe { &mut *rows.cast::<Vec<String>>() };
            if ncols > 0 {
                let v = unsafe { *values };
                let s = if v.is_null() {
                    String::new()
                } else {
                    unsafe { CStr::from_ptr(v) }.to_string_lossy().into_owned()
                };
                rows.push(s);
            }
            0
        }
        let sql = CString::new(sql).unwrap();
        let mut rows: Vec<String> = Vec::new();
        let mut err: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            ffi::sqlite3_exec(
                self.0,
                sql.as_ptr(),
                Some(collect),
                (&mut rows as *mut Vec<String>).cast(),
                &mut err,
            )
        };
        if rc == ffi::SQLITE_OK {
            return Ok(rows);
        }
        let msg = if err.is_null() {
            String::new()
        } else {
            let s = unsafe { CStr::from_ptr(err) }.to_string_lossy().into_owned();
            unsafe { ffi::sqlite3_free(err.cast()) };
            s
        };
        Err((rc, msg))
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        unsafe { ffi::sqlite3_close(self.0) };
    }
}

fn fixture() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/openssl-provider.db"))
}

#[test]
fn opens_openssl_provider_db() {
    let db = Db::open(fixture(), ffi::SQLITE_OPEN_READONLY);
    db.query("PRAGMA key='k'").unwrap();
    assert_eq!(db.query("SELECT a FROM t").unwrap(), ["1"]);
    assert_eq!(db.query("PRAGMA cipher_provider").unwrap(), ["openssl"]);
    assert!(
        db.query("PRAGMA cipher_provider_version").unwrap()[0].starts_with("BoringSSL"),
        "provider is not BoringSSL"
    );
}

#[test]
fn wrong_key_is_not_a_database() {
    let db = Db::open(fixture(), ffi::SQLITE_OPEN_READONLY);
    db.query("PRAGMA key='wrong'").unwrap();
    let (rc, msg) = db.query("SELECT a FROM t").unwrap_err();
    assert_eq!(rc, ffi::SQLITE_NOTADB);
    assert_eq!(msg, "file is not a database");
}

#[test]
fn writes_readable_sqlcipher_db() {
    let path = std::env::temp_dir().join(format!("sqlcipher-boringssl-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    {
        let db = Db::open(&path, ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE);
        db.query("PRAGMA key='k'").unwrap();
        db.query("CREATE TABLE t(a); INSERT INTO t VALUES(1)").unwrap();
        assert_eq!(db.query("PRAGMA cipher_version").unwrap(), ["4.10.0 community"]);
    }
    // Encrypted at rest: no SQLite magic.
    assert_ne!(&std::fs::read(&path).unwrap()[..15], b"SQLite format 3");
    let db = Db::open(&path, ffi::SQLITE_OPEN_READONLY);
    db.query("PRAGMA key='k'").unwrap();
    assert_eq!(db.query("SELECT a FROM t").unwrap(), ["1"]);
    drop(db);
    std::fs::remove_file(&path).unwrap();
}
