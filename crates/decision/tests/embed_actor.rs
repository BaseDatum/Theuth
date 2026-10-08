//! The `embed` actor (144 §7.2) over the tiny random-weight checkpoint the test writes: info,
//! token counts, batched unit vectors, idle unload, and the missing-directory fallback. No download.

use std::time::{Duration, Instant};

use decision::candle::embed::{BATCH, TINY_WORDS, write_tiny_checkpoint};
use decision::embed::{Embed, EmbedActor, Info, IsLoaded, Tokens, check_name, model_dir};
use kameo::actor::Spawn;

#[tokio::test]
async fn tiny_checkpoint_round_trip_and_idle_unload() {
    let dir = tempfile::tempdir().unwrap();
    let model = model_dir(dir.path(), "tiny").unwrap();
    write_tiny_checkpoint(&model).unwrap();
    let actor = EmbedActor::spawn(
        EmbedActor::new(model, decision::candle::Gpu::None).with_idle(Duration::from_millis(150)),
    );

    let info = actor.ask(Info).await.unwrap().expect("installed");
    assert_eq!((info.model.as_str(), info.dims), ("tiny", 64));
    assert!(
        !actor.ask(IsLoaded).await.unwrap(),
        "Info never loads weights"
    );

    let n = actor
        .ask(Tokens {
            texts: vec!["failover cluster alpha".into(), "".into()],
        })
        .await
        .unwrap();
    assert_eq!(n, [5, 2], "[CLS] + words + [SEP]");
    assert!(
        !actor.ask(IsLoaded).await.unwrap(),
        "Tokens never loads weights"
    );

    let texts: Vec<String> = (0..BATCH + 1)
        .map(|i| format!("database replica {}", ["one", "two"][i % 2]))
        .collect();
    let v = actor.ask(Embed { texts }).await.unwrap();
    assert_eq!(v.len(), BATCH + 1);
    assert!(v.iter().all(|r| r.len() == 64));
    for r in &v {
        let norm: f32 = r.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3);
    }
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    assert!(dot(&v[0], &v[2]) > 0.9999, "same text, same vector");
    assert!(
        dot(&v[0], &v[BATCH]) > 0.9999,
        "…across the batch boundary too"
    );
    assert!(
        dot(&v[0], &v[1]) < 0.9999,
        "different text, different vector"
    );
    assert!(actor.ask(IsLoaded).await.unwrap());

    // Cost note for the as-built: one 400-token chunk on the CPU with the tiny model.
    let words: Vec<&str> = TINY_WORDS.split_whitespace().collect();
    let chunk: String = (0..398)
        .map(|i| words[i % words.len()])
        .collect::<Vec<_>>()
        .join(" ");
    let t = Instant::now();
    let out = actor
        .ask(Embed {
            texts: vec![chunk; 8],
        })
        .await
        .unwrap();
    assert_eq!(out.len(), 8);
    eprintln!(
        "embed: tiny model, CPU f32: {:.1} ms per 400-token chunk (batch of 8)",
        t.elapsed().as_secs_f64() * 1000.0 / 8.0
    );

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!actor.ask(IsLoaded).await.unwrap(), "idle → released");
}

#[tokio::test]
async fn missing_directory_is_lexical_only_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let actor = EmbedActor::spawn(EmbedActor::new(
        model_dir(dir.path(), "embed-gte-modernbert-base").unwrap(),
        decision::candle::Gpu::None,
    ));
    assert_eq!(actor.ask(Info).await.unwrap(), None);
    let e = actor
        .ask(Embed {
            texts: vec!["x".into()],
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("not installed"), "{e}");
    assert!(
        actor
            .ask(Tokens {
                texts: vec!["x".into()]
            })
            .await
            .is_err()
    );
}

#[test]
fn model_name_rule() {
    assert!(check_name("embed-gte-modernbert-base").is_ok());
    for bad in ["Alibaba-NLP/gte-modernbert-base", "", "..", "a\\b"] {
        let e = check_name(bad).unwrap_err().to_string();
        assert!(e.contains("never provider/model"), "{bad}: {e}");
    }
}
