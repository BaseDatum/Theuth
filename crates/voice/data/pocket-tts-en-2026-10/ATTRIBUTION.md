# Pocket TTS english_2026-10 — attribution

The spoken-reply model is **Kyutai Pocket TTS**, English checkpoint `languages/english_2026-10`
(2026-10-21) from <https://huggingface.co/kyutai/pocket-tts-without-voice-cloning>, revision
`a7056f505a4a8ca72d8b5fb1440071d6f1a35739`, licensed
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Code: <https://github.com/kyutai-labs/pocket-tts>.

- `model.safetensors`, `tokenizer.json`: Kyutai's files, unmodified (fetched by
  `scripts/fetch-models.sh`).
- `voices/<name>.safetensors`: Kyutai's `embeddings/<name>.safetensors` voice states, converted to
  the `emb` voice files ptts reads; row 0 is the checkpoint's `flow_lm.bos_before_voice`
  (`crates/voice/build-voices/from_state.py`).
- `config.json`: ours (the checkpoint ships none).

Voices:

- `peter_yearsley` — Peter Yearsley, a LibriVox recording selected by
  [Voice-Zero](https://github.com/OwenTyme/voice-zero);
  source <https://huggingface.co/kyutai/tts-voices/blob/main/voice-zero/peter_yearsley.wav>.
- `alba` — Alba MacKenna, CC BY 4.0;
  source <https://huggingface.co/kyutai/tts-voices/blob/main/alba-mackenna/casual.wav>.
