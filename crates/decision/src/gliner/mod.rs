//! GLiNER2.5-Decide backend (`Kind::Gliner`): DeBERTa-v3-large + a per-label head that scores
//! one logit per label — its own tokenizer and prompt layout, nothing of laya's `[MASK]`
//! rendering. **Stub until GLINER-3**: [`Loaded::open`] refuses to load, so every ask on a
//! gliner checkpoint escalates `HardError("load: gliner backend not built yet (GLINER-3)")`.
//! What GLINER-3 fills in: `open` (weights + `tokenizer.json` + `config.json`, see
//! [`crate::engine::model_files`]), `build` (render the question's labels and state), `run`
//! (one forward, sigmoid/softmax per label into [`Scores::probs`] in [`crate::laya::labels`]
//! order).

use std::any::Any;
use std::path::Path;
use std::time::Duration;

use crate::DecisionConfig;
use crate::engine::{ModelSha, Oracle, Row, Scores};
use crate::question::Question;

const NOT_BUILT: &str = "gliner backend not built yet (GLINER-3)";

/// One rendered question for the gliner head: token ids plus the 57 §0.5 flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    /// Token ids as the DeBERTa tokenizer produced them.
    pub input_ids: Vec<i64>,
    /// The state was right-truncated to the length cap.
    pub state_cut: bool,
    /// A label fell past the length cap and cannot be scored.
    pub labels_lost: bool,
}

impl Row for Rendered {
    fn truncated(&self) -> bool {
        self.state_cut || self.labels_lost
    }
    fn state_cut(&self) -> bool {
        self.state_cut
    }
    fn markers_lost(&self) -> bool {
        self.labels_lost
    }
    fn ids(&self) -> &[i64] {
        &self.input_ids
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The gliner session. Never constructed until GLINER-3 lands `open`.
pub struct Loaded {
    model_sha: ModelSha,
    load_time: Duration,
}

impl Loaded {
    /// Open the checkpoint in `dir`. GLINER-3.
    pub fn open(dir: &Path, _cfg: &DecisionConfig) -> anyhow::Result<Loaded> {
        anyhow::bail!("{NOT_BUILT}: {}", dir.display())
    }
}

impl Oracle for Loaded {
    fn model_sha(&self) -> ModelSha {
        self.model_sha.clone()
    }
    fn build(&self, _q: &Question) -> Result<Box<dyn Row>, String> {
        Err(NOT_BUILT.to_owned())
    }
    fn run(&mut self, _rows: &[Box<dyn Row>]) -> anyhow::Result<Vec<Scores>> {
        anyhow::bail!(NOT_BUILT)
    }
    fn load_time(&self) -> Duration {
        self.load_time
    }
}
