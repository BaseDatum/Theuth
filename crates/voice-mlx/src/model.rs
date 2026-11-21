//! FastConformer encoder + LSTM prediction net + joint + greedy TDT decoding, ported from parakeet-mlx 0.7.0
//! (`conformer.py`, `attention.py`, `rnnt.py`, `parakeet.py::ParakeetTDT.decode_greedy`). Weight names and
//! layouts are those of the mlx-community safetensors (convs already OHWI / `[out, k, in/groups]`, so no
//! transposes). Weights stay in the dtype they ship in (float32); activations are float32 throughout.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use mlx_rs::ops::indexing::{IndexOp, argmax_axis};
use mlx_rs::{Array, fast, nn, ops};
use serde_json::Value;

use crate::Transcript;
use crate::mel;
use crate::text::{Token, assemble};

const LN_EPS: f32 = 1e-5;
const BN_EPS: f32 = 1e-5;

struct Config {
    n_layers: usize,
    d_model: i32,
    n_heads: i32,
    conv_kernel: i32,
    subsampling_factor: usize,
    pred_layers: usize,
    vocabulary: Vec<String>,
    durations: Vec<usize>,
    max_symbols: Option<usize>,
}

impl Config {
    fn parse(v: &Value) -> Result<Self> {
        let s = |p: &str| {
            v.pointer(p)
                .and_then(Value::as_str)
                .with_context(|| format!("config.json: {p}"))
        };
        let u = |p: &str| {
            v.pointer(p)
                .and_then(Value::as_u64)
                .with_context(|| format!("config.json: {p}"))
        };
        let f = |p: &str| {
            v.pointer(p)
                .and_then(Value::as_f64)
                .with_context(|| format!("config.json: {p}"))
        };
        // The front end and attention are ported for exactly this shape of model; refuse anything else.
        ensure!(
            s("/target")? == "nemo.collections.asr.models.rnnt_bpe_models.EncDecRNNTBPEModel",
            "not an RNNT/TDT model"
        );
        ensure!(s("/decoding/model_type")? == "tdt", "not a TDT model");
        ensure!(
            s("/encoder/self_attention_model")? == "rel_pos",
            "only rel_pos attention is supported"
        );
        ensure!(
            s("/encoder/subsampling")? == "dw_striding",
            "only dw_striding subsampling is supported"
        );
        ensure!(
            v.pointer("/encoder/xscaling").and_then(Value::as_bool) != Some(true),
            "xscaling is not supported"
        );
        ensure!(
            s("/preprocessor/normalize")? == "per_feature" && s("/preprocessor/window")? == "hann",
            "preprocessor"
        );
        ensure!(
            u("/preprocessor/sample_rate")? as usize == mel::SAMPLE_RATE
                && u("/preprocessor/features")? as usize == mel::N_MELS
                && u("/preprocessor/n_fft")? as usize == mel::N_FFT
                && (f("/preprocessor/window_size")? * mel::SAMPLE_RATE as f64) as usize == mel::WIN
                && (f("/preprocessor/window_stride")? * mel::SAMPLE_RATE as f64) as usize
                    == mel::HOP,
            "unsupported preprocessor settings"
        );
        let vocabulary: Vec<String> = v
            .pointer("/joint/vocabulary")
            .and_then(Value::as_array)
            .context("config.json: /joint/vocabulary")?
            .iter()
            .map(|t| t.as_str().map(str::to_owned).context("vocabulary entry"))
            .collect::<Result<_>>()?;
        let durations: Vec<usize> = v
            .pointer("/decoding/durations")
            .and_then(Value::as_array)
            .context("config.json: /decoding/durations")?
            .iter()
            .map(|d| d.as_u64().map(|d| d as usize).context("duration"))
            .collect::<Result<_>>()?;
        Ok(Self {
            n_layers: u("/encoder/n_layers")? as usize,
            d_model: u("/encoder/d_model")? as i32,
            n_heads: u("/encoder/n_heads")? as i32,
            conv_kernel: u("/encoder/conv_kernel_size")? as i32,
            subsampling_factor: u("/encoder/subsampling_factor")? as usize,
            pred_layers: u("/decoder/prednet/pred_rnn_layers")? as usize,
            vocabulary,
            durations,
            max_symbols: v
                .pointer("/decoding/greedy/max_symbols")
                .and_then(Value::as_u64)
                .map(|m| m as usize),
        })
    }
}

/// Parakeet TDT on MLX. MLX arrays are bound to the thread that made them: keep this on one OS thread.
pub struct ParakeetMlx {
    cfg: Config,
    w: HashMap<String, Array>,
    fb: Array,
}

impl ParakeetMlx {
    /// Load `config.json` and `model.safetensors` from `dir` (see [`crate::FILES`]); materialises all weights.
    pub fn load(dir: &Path) -> Result<Self> {
        let cfg_path = dir.join("config.json");
        let raw =
            std::fs::read(&cfg_path).with_context(|| format!("reading {}", cfg_path.display()))?;
        let cfg = Config::parse(&serde_json::from_slice(&raw)?)?;
        let st = dir.join("model.safetensors");
        let w =
            Array::load_safetensors(&st).with_context(|| format!("loading {}", st.display()))?;
        let model = Self {
            cfg,
            w,
            fb: mel::filterbank_array(),
        };
        model.check_weights()?;
        let all: Vec<&Array> = model.w.values().collect();
        mlx_rs::transforms::eval(all)?;
        Ok(model)
    }

    fn check_weights(&self) -> Result<()> {
        let mut need = vec![
            "encoder.pre_encode.out.weight".to_owned(),
            "decoder.prediction.embed.weight".into(),
            "joint.joint_net.2.weight".into(),
        ];
        need.extend(
            (0..self.cfg.n_layers).map(|l| format!("encoder.layers.{l}.self_attn.pos_bias_u")),
        );
        need.extend(
            (0..self.cfg.pred_layers).map(|l| format!("decoder.prediction.dec_rnn.lstm.{l}.Wx")),
        );
        for n in need {
            ensure!(self.w.contains_key(&n), "model.safetensors lacks {n}");
        }
        let classes = self.w["joint.joint_net.2.weight"].shape()[0] as usize;
        ensure!(
            classes == self.cfg.vocabulary.len() + 1 + self.cfg.durations.len(),
            "joint output {classes} != vocabulary + blank + durations"
        );
        Ok(())
    }

    /// One short silent clip, to compile/cache the Metal kernels before the first real request.
    pub fn warm(&mut self) -> Result<()> {
        self.transcribe(&vec![0.0; mel::SAMPLE_RATE])?;
        Ok(())
    }

    /// Transcribe 16 kHz mono float PCM in one pass (parakeet-mlx's default, `chunk_duration=None`).
    pub fn transcribe(&mut self, pcm16k: &[f32]) -> Result<Transcript> {
        let mel = mel::log_mel(pcm16k, &self.fb)?;
        let enc = self.encode(&mel)?;
        let tokens = self.decode_greedy(&enc)?;
        let seconds_per_frame =
            (self.cfg.subsampling_factor * mel::HOP) as f32 / mel::SAMPLE_RATE as f32;
        Ok(assemble(&tokens, &self.cfg.vocabulary, seconds_per_frame))
    }

    fn p(&self, name: &str) -> Result<&Array> {
        self.w
            .get(name)
            .with_context(|| format!("missing weight {name}"))
    }

    /// `nn.Linear`: `x · Wᵀ (+ b)`.
    fn linear(&self, x: &Array, prefix: &str) -> Result<Array> {
        let y = ops::matmul(x, self.p(&format!("{prefix}.weight"))?.t())?;
        match self.w.get(&format!("{prefix}.bias")) {
            Some(b) => Ok(y.add(b)?),
            None => Ok(y),
        }
    }

    fn layer_norm(&self, x: &Array, prefix: &str) -> Result<Array> {
        Ok(fast::layer_norm(
            x,
            self.p(&format!("{prefix}.weight"))?,
            self.p(&format!("{prefix}.bias"))?,
            LN_EPS,
        )?)
    }

    fn conv2d(
        &self,
        x: &Array,
        prefix: &str,
        stride: i32,
        padding: i32,
        groups: i32,
    ) -> Result<Array> {
        let y = ops::conv2d(
            x,
            self.p(&format!("{prefix}.weight"))?,
            (stride, stride),
            (padding, padding),
            None,
            groups,
        )?;
        Ok(y.add(self.p(&format!("{prefix}.bias"))?)?)
    }

    /// `Conformer.__call__` for one utterance: `[1, T, 128]` mel → `[1, T/8, d_model]`.
    fn encode(&self, mel: &Array) -> Result<Array> {
        // DwStridingSubsampling: NHWC with H = time, W = mel bins, C = 1.
        let x = ops::expand_dims(mel, 3)?;
        let x = nn::relu(self.conv2d(&x, "encoder.pre_encode.conv.0", 2, 1, 1)?)?;
        let ch = x.dim(3);
        let x = self.conv2d(&x, "encoder.pre_encode.conv.2", 2, 1, ch)?;
        let x = nn::relu(self.conv2d(&x, "encoder.pre_encode.conv.3", 1, 0, 1)?)?;
        let x = self.conv2d(&x, "encoder.pre_encode.conv.5", 2, 1, ch)?;
        let x = nn::relu(self.conv2d(&x, "encoder.pre_encode.conv.6", 1, 0, 1)?)?;
        // [1, T', F', C] → [1, T', C, F'] → [1, T', C·F'] (channel-major, like torch's NCHW flatten).
        let (t, f) = (x.dim(1), x.dim(2));
        let x = x.transpose_axes(&[0, 1, 3, 2])?.reshape(&[1, t, ch * f])?;
        let mut x = self.linear(&x, "encoder.pre_encode.out")?;

        let pos_emb = rel_pos_emb(t, self.cfg.d_model);
        for l in 0..self.cfg.n_layers {
            x = self.conformer_block(&x, &pos_emb, &format!("encoder.layers.{l}"))?;
        }
        x.eval()?;
        Ok(x)
    }

    fn feed_forward(&self, x: &Array, prefix: &str) -> Result<Array> {
        let h = nn::silu(self.linear(x, &format!("{prefix}.linear1"))?)?;
        self.linear(&h, &format!("{prefix}.linear2"))
    }

    fn conformer_block(&self, x: &Array, pos_emb: &Array, pre: &str) -> Result<Array> {
        let half = Array::from_f32(0.5);
        let ff1 = self.feed_forward(
            &self.layer_norm(x, &format!("{pre}.norm_feed_forward1"))?,
            &format!("{pre}.feed_forward1"),
        )?;
        let x = x.add(ff1.multiply(&half)?)?;
        let xn = self.layer_norm(&x, &format!("{pre}.norm_self_att"))?;
        let x = x.add(self.rel_attention(&xn, pos_emb, &format!("{pre}.self_attn"))?)?;
        let xn = self.layer_norm(&x, &format!("{pre}.norm_conv"))?;
        let x = x.add(self.conv_module(&xn, &format!("{pre}.conv"))?)?;
        let ff2 = self.feed_forward(
            &self.layer_norm(&x, &format!("{pre}.norm_feed_forward2"))?,
            &format!("{pre}.feed_forward2"),
        )?;
        let x = x.add(ff2.multiply(&half)?)?;
        self.layer_norm(&x, &format!("{pre}.norm_out"))
    }

    /// `RelPositionMultiHeadAttention` (Transformer-XL style, untied u/v biases, rel_shift on the BD term).
    fn rel_attention(&self, x: &Array, pos_emb: &Array, pre: &str) -> Result<Array> {
        let (h, dk) = (self.cfg.n_heads, self.cfg.d_model / self.cfg.n_heads);
        let t = x.dim(1);
        let q = self
            .linear(x, &format!("{pre}.linear_q"))?
            .reshape(&[1, t, h, dk])?;
        let k = self
            .linear(x, &format!("{pre}.linear_k"))?
            .reshape(&[1, t, h, dk])?
            .transpose_axes(&[0, 2, 1, 3])?;
        let v = self
            .linear(x, &format!("{pre}.linear_v"))?
            .reshape(&[1, t, h, dk])?
            .transpose_axes(&[0, 2, 1, 3])?;
        let p = self.linear(pos_emb, &format!("{pre}.linear_pos"))?;
        let pos_len = p.dim(1);
        let p = p
            .reshape(&[1, pos_len, h, dk])?
            .transpose_axes(&[0, 2, 1, 3])?;
        let q_u = q
            .add(self.p(&format!("{pre}.pos_bias_u"))?)?
            .transpose_axes(&[0, 2, 1, 3])?;
        let q_v = q
            .add(self.p(&format!("{pre}.pos_bias_v"))?)?
            .transpose_axes(&[0, 2, 1, 3])?;

        let scale = (dk as f32).powf(-0.5);
        let bd = ops::matmul(&q_v, p.swap_axes(-2, -1)?)?; // [1, h, t, pos_len]
        // rel_shift: pad one column on the left, fold, drop the first row, unfold.
        let bd = ops::pad(&bd, &[(0, 0), (0, 0), (0, 0), (1, 0)][..], None, None)?;
        let bd = bd
            .reshape(&[1, h, pos_len + 1, t])?
            .index((.., .., 1.., ..))
            .reshape(&[1, h, t, pos_len])?;
        let bd = bd
            .index((.., .., .., ..t))
            .multiply(Array::from_f32(scale))?;

        let o = fast::scaled_dot_product_attention(&q_u, &k, &v, scale, &bd, None)?;
        let o = o.transpose_axes(&[0, 2, 1, 3])?.reshape(&[1, t, h * dk])?;
        self.linear(&o, &format!("{pre}.linear_out"))
    }

    /// Conformer convolution module: pointwise → GLU → depthwise (same padding) → BatchNorm (eval) → SiLU → pointwise.
    fn conv_module(&self, x: &Array, pre: &str) -> Result<Array> {
        let y = ops::conv1d(
            x,
            self.p(&format!("{pre}.pointwise_conv1.weight"))?,
            None,
            None,
            None,
            None,
        )?;
        let halves = y.split_equal(2, -1)?;
        let y = halves[0].multiply(ops::sigmoid(&halves[1])?)?;
        let pad = (self.cfg.conv_kernel - 1) / 2;
        let y = ops::pad(&y, &[(0, 0), (pad, pad), (0, 0)][..], None, None)?;
        let y = ops::conv1d(
            &y,
            self.p(&format!("{pre}.depthwise_conv.weight"))?,
            None,
            None,
            None,
            self.cfg.d_model,
        )?;
        let bn = |n: &str| self.p(&format!("{pre}.batch_norm.{n}"));
        let y = y
            .subtract(bn("running_mean")?)?
            .multiply(bn("running_var")?.add(Array::from_f32(BN_EPS))?.rsqrt()?)?
            .multiply(bn("weight")?)?
            .add(bn("bias")?)?;
        let y = nn::silu(y)?;
        Ok(ops::conv1d(
            &y,
            self.p(&format!("{pre}.pointwise_conv2.weight"))?,
            None,
            None,
            None,
            None,
        )?)
    }

    /// One step of the 2-layer LSTM prediction net (`mlx.nn.LSTM` gate order i, f, g, o).
    fn predict(
        &self,
        token: Option<usize>,
        state: &[(Array, Array)],
    ) -> Result<(Array, Vec<(Array, Array)>)> {
        let mut x = match token {
            Some(id) => self
                .p("decoder.prediction.embed.weight")?
                .index(id as i32)
                .reshape(&[1, -1])?,
            None => {
                let hidden = self.p("decoder.prediction.embed.weight")?.dim(1);
                Array::zeros::<f32>(&[1, hidden])?
            }
        };
        let mut next = Vec::with_capacity(state.len());
        for (l, (h, c)) in state.iter().enumerate() {
            let pre = format!("decoder.prediction.dec_rnn.lstm.{l}");
            let gates = ops::matmul(&x, self.p(&format!("{pre}.Wx"))?.t())?
                .add(self.p(&format!("{pre}.bias"))?)?
                .add(ops::matmul(h, self.p(&format!("{pre}.Wh"))?.t())?)?;
            let g = gates.split_equal(4, -1)?;
            let (i, f, gg, o) = (
                ops::sigmoid(&g[0])?,
                ops::sigmoid(&g[1])?,
                ops::tanh(&g[2])?,
                ops::sigmoid(&g[3])?,
            );
            let c2 = f.multiply(c)?.add(i.multiply(&gg)?)?;
            let h2 = o.multiply(ops::tanh(&c2)?)?;
            x = h2.clone();
            next.push((h2, c2));
        }
        Ok((x, next))
    }

    /// `ParakeetTDT.decode_greedy` for batch 1: argmax token, argmax duration, advance by the duration, at most
    /// `max_symbols` consecutive zero-duration emissions per frame.
    fn decode_greedy(&self, enc: &Array) -> Result<Vec<Token>> {
        let frames = enc.dim(1) as usize;
        let enc_proj = self.linear(&enc.index(0), "joint.enc")?; // [T, joint_hidden]; joint.enc is per-frame
        enc_proj.eval()?;
        let vocab = self.cfg.vocabulary.len() as i32;
        let hidden = self.p("decoder.prediction.embed.weight")?.dim(1);
        let zero = || Array::zeros::<f32>(&[1, hidden]);
        let mut state: Vec<(Array, Array)> = (0..self.cfg.pred_layers)
            .map(|_| Ok((zero()?, zero()?)))
            .collect::<Result<_>>()?;
        let mut last: Option<usize> = None;
        // The prediction net only changes when a token is emitted; cache its output (same values as recomputing).
        let (mut pred_out, mut pred_state) = self.predict(last, &state)?;
        let mut pred_proj = self.linear(&pred_out, "joint.pred")?;

        let mut out = Vec::new();
        let (mut step, mut new_symbols) = (0usize, 0usize);
        while step < frames {
            let joint = nn::relu(enc_proj.index(step as i32).add(&pred_proj)?)?;
            let logits = self.linear(&joint, "joint.joint_net.2")?.reshape(&[-1])?;
            let tok = argmax_axis(logits.index(..vocab + 1), 0, None)?;
            let dur = argmax_axis(logits.index(vocab + 1..), 0, None)?;
            mlx_rs::transforms::eval([&tok, &dur])?;
            let (tok, dur) = (tok.item_exact::<u32>() as usize, dur.item_exact::<u32>() as usize);
            let Some(&duration) = self.cfg.durations.get(dur) else {
                bail!("duration index {dur} out of range")
            };

            if tok != vocab as usize {
                out.push(Token {
                    id: tok,
                    start_frame: step,
                    duration_frames: duration,
                });
                last = Some(tok);
                state = pred_state;
                (pred_out, pred_state) = self.predict(last, &state)?;
                pred_proj = self.linear(&pred_out, "joint.pred")?;
            }
            step += duration;
            new_symbols += 1;
            if duration != 0 {
                new_symbols = 0;
            } else if self.cfg.max_symbols.is_some_and(|m| m <= new_symbols) {
                step += 1;
                new_symbols = 0;
            }
        }
        Ok(out)
    }
}

/// `RelPositionalEncoding` slice for length `t`: positions `t-1 … -(t-1)`, interleaved sin/cos, `[1, 2t-1, d]`.
fn rel_pos_emb(t: i32, d: i32) -> Array {
    // parakeet-mlx: `exp(arange(0, d, 2, f32) * -(log(10000) / d))`, the factor a Python float rounded to f32.
    let half = (d / 2) as usize;
    let k = -(10000f64.ln() / d as f64) as f32;
    let div: Vec<f32> = (0..half).map(|i| ((2 * i) as f32 * k).exp()).collect();
    let n = (2 * t - 1) as usize;
    let mut pe = vec![0f32; n * d as usize];
    for r in 0..n {
        let pos = (t - 1 - r as i32) as f32;
        for (i, dv) in div.iter().enumerate() {
            pe[r * d as usize + 2 * i] = (pos * dv).sin();
            pe[r * d as usize + 2 * i + 1] = (pos * dv).cos();
        }
    }
    Array::from_slice(&pe, &[1, n as i32, d])
}
