//! Owner: B2 — `t plugin new` / `plugin_new`: a plugin folder from the bundled SDK templates
//! (contract §10.2). Public for [`new_plugin`] (contract §3.4).

use crate::error::PluginsError;
use crate::types::{Lang, ScaffoldReport};

/// Write `<plugins_dir>/<name>/` from the bundled SDK template for `lang` (raw: the no-SDK
/// template; only `py` and `sh` have one). Fails with `Exists` if the folder exists.
pub fn new_plugin(
    plugins_dir: &std::path::Path,
    name: &str,
    lang: Lang,
    raw: bool,
) -> Result<ScaffoldReport, PluginsError> {
    let _ = (plugins_dir, name, lang, raw);
    Err(PluginsError::not_implemented("scaffold::new_plugin"))
}
