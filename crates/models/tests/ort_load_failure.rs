//! `ort`'s loader cell completes even when its dlopen fails, so a second `ort::init_from`
//! reports success over a null library handle and the first session panics in
//! `dlsym(0x0, OrtGetApiBase)` (the ORT-INIT-RACE launch-2 shape). `ort_lib` must therefore
//! hand every later caller the load error instead of a path. Process-wide state: this test
//! binary holds only this test, and it never loads a real runtime.

#![cfg(not(target_os = "ios"))]

use std::path::PathBuf;

fn scratch_data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("models-ort-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("models").join(models::onnx::DIR)).unwrap();
    dir
}

#[test]
fn a_failed_load_is_final_for_the_process() {
    let bad = scratch_data_dir("bad");
    std::fs::write(
        models::onnx::runtime_path(&bad),
        b"not a mach-o / elf image",
    )
    .unwrap();
    let good = scratch_data_dir("good");

    // The search misses: not a load, not cached. A box with `$ORT_DYLIB_PATH` or a Homebrew
    // runtime loads it for real here and cannot exercise the failure.
    let missing = match models::onnx::ort_lib(&good) {
        Ok(lib) => {
            eprintln!(
                "skipped: a runtime is installed globally at {}",
                lib.display()
            );
            return;
        }
        Err(e) => e,
    };
    assert!(missing.starts_with("no "), "{missing}");
    assert!(!models::onnx::ort_ready());

    // The load fails: cached, and named for what it is.
    let failed = models::onnx::ort_lib(&bad).unwrap_err();
    assert!(failed.starts_with("load "), "{failed}");
    assert!(!models::onnx::ort_ready());

    // Whatever the next caller offers — the same file, or a file that appeared since — the answer
    // is the recorded failure, never a path to a runtime `ort` cannot actually reach.
    assert_eq!(models::onnx::ort_lib(&bad).unwrap_err(), failed);
    std::fs::write(models::onnx::runtime_path(&good), b"also not an image").unwrap();
    assert_eq!(models::onnx::ort_lib(&good).unwrap_err(), failed);
    assert!(!models::onnx::ort_ready());
    assert_eq!(models::onnx::ep(), models::onnx::CPU_EP);
    assert!(models::onnx::devices().is_empty());

    let _ = std::fs::remove_dir_all(&bad);
    let _ = std::fs::remove_dir_all(&good);
}
